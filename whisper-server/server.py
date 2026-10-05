#!/usr/bin/env python3
"""
FlowCapture Whisper Server (faster-whisper)
Server locale compatibile con l'API OpenAI (/v1/audio/transcriptions e /audio/transcriptions)
per la trascrizione audio ad alte prestazioni con faster-whisper (CTranslate2).
"""

import os
import sys
import tempfile
import argparse
import logging
from typing import Optional, Dict, List

# Su Windows con Python >= 3.8, registra esplicitamente le cartelle dei pacchetti nvidia
# contenenti le librerie CUDA (cublas64_12.dll, cudnn64_9.dll, nvrtc64_120_0.dll, ecc.)
if sys.platform == "win32":
    import site
    search_dirs = []
    sp_roots = []
    try:
        sp_roots.extend(site.getsitepackages())
    except Exception:
        pass
    try:
        user_sp = site.getusersitepackages()
        if user_sp and user_sp not in sp_roots:
            sp_roots.append(user_sp)
    except Exception:
        pass

    for sp in sp_roots:
        if not os.path.isdir(sp):
            continue
        nvidia_root = os.path.join(sp, "nvidia")
        if os.path.isdir(nvidia_root):
            try:
                for sub in os.listdir(nvidia_root):
                    bin_dir = os.path.join(nvidia_root, sub, "bin")
                    if os.path.isdir(bin_dir):
                        search_dirs.append(bin_dir)
            except Exception:
                pass
        torch_lib = os.path.join(sp, "torch", "lib")
        if os.path.isdir(torch_lib):
            search_dirs.append(torch_lib)

    cuda_path = os.environ.get("CUDA_PATH", "")
    if cuda_path:
        search_dirs.append(os.path.join(cuda_path, "bin"))

    for d in search_dirs:
        try:
            os.add_dll_directory(d)
        except Exception:
            pass
        os.environ["PATH"] = d + os.pathsep + os.environ.get("PATH", "")

from fastapi import FastAPI, UploadFile, File, Form, HTTPException
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse
import uvicorn

from faster_whisper import WhisperModel

# Configurazione logging
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
    datefmt="%H:%M:%S",
)
logger = logging.getLogger("whisper-server")

app = FastAPI(
    title="FlowCapture Whisper Server",
    description="Server di trascrizione audio locale compatibile con le API di FlowCapture e OpenAI Whisper",
    version="1.0.0",
)

# Abilita CORS per richieste da qualsiasi client/Tauri app
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

# Cache dei modelli caricati
models_cache: Dict[str, WhisperModel] = {}
default_model_name = os.environ.get("WHISPER_MODEL", "small")
configured_device = os.environ.get("WHISPER_DEVICE", "auto")
configured_compute_type = os.environ.get("WHISPER_COMPUTE_TYPE", "auto")


def get_optimal_device_and_compute():
    """Rileva automaticamente se CUDA (GPU Nvidia) e' disponibile, altrimenti usa CPU."""
    device = configured_device
    compute_type = configured_compute_type

    if device == "auto":
        try:
            import torch
            if torch.cuda.is_available():
                device = "cuda"
            else:
                device = "cpu"
        except ImportError:
            device = "cpu"

    if compute_type == "auto":
        if device == "cuda":
            compute_type = "float16"
        else:
            # int8 e' il compute type ottimale per CPU con CTranslate2 (2-4x piu' veloce)
            compute_type = "int8"

    return device, compute_type


def get_whisper_model(model_name: Optional[str] = None) -> WhisperModel:
    """Carica o recupera dalla cache il modello faster-whisper specificato."""
    name = (model_name or default_model_name or "small").strip().lower()
    
    # Mappa alias comuni (es. whisper-1 -> default_model)
    if name in ["whisper-1", "whisper", "default", ""]:
        name = default_model_name

    if name in models_cache:
        return models_cache[name]

    device, compute_type = get_optimal_device_and_compute()
    logger.info(f"Caricamento modello faster-whisper: '{name}' (device={device}, compute_type={compute_type})...")
    
    try:
        model = WhisperModel(name, device=device, compute_type=compute_type)
        models_cache[name] = model
        logger.info(f"Modello '{name}' caricato con successo!")
        return model
    except Exception as e:
        logger.error(f"Errore durante il caricamento del modello '{name}': {e}")
        # Se fallisce su GPU o int8, prova fallback su CPU default
        if device != "cpu" or compute_type != "default":
            logger.info(f"Tentativo fallback su CPU con compute_type=default...")
            model = WhisperModel(name, device="cpu", compute_type="default")
            models_cache[name] = model
            logger.info(f"Modello '{name}' caricato con fallback CPU.")
            return model
        raise


@app.get("/")
def root():
    device, compute_type = get_optimal_device_and_compute()
    return {
        "service": "FlowCapture Whisper Server",
        "engine": "faster-whisper",
        "default_model": default_model_name,
        "device": device,
        "compute_type": compute_type,
        "status": "ready",
        "endpoints": [
            "POST /v1/audio/transcriptions",
            "POST /audio/transcriptions",
            "POST /v1/audio/transcriptions/segments  (with timed segments + confidence)",
            "POST /audio/transcriptions/segments     (with timed segments + confidence)",
        ],
    }


@app.get("/health")
def health():
    return {"status": "ok"}


@app.get("/v1/models")
def list_models():
    return {
        "data": [
            {"id": "tiny", "object": "model"},
            {"id": "base", "object": "model"},
            {"id": "small", "object": "model"},
            {"id": "medium", "object": "model"},
            {"id": "large-v2", "object": "model"},
            {"id": "large-v3", "object": "model"},
            {"id": "whisper-1", "object": "model"},
        ]
    }


async def handle_transcription(
    file: UploadFile,
    model: Optional[str],
    language: Optional[str],
    temperature: Optional[float],
    prompt: Optional[str],
    include_segments: bool = False,
):
    """Elabora la trascrizione del file audio usando faster-whisper."""
    if not file:
        raise HTTPException(status_code=400, detail="Nessun file audio inviato.")

    # Normalizza lingua
    clean_language: Optional[str] = None
    if language:
        lang_str = language.strip().lower()
        if lang_str and lang_str != "auto":
            clean_language = lang_str

    # Salva il file audio ricevuto in un file temporaneo su disco
    suffix = os.path.splitext(file.filename or "")[1] or ".webm"
    with tempfile.NamedTemporaryFile(delete=False, suffix=suffix) as temp_audio:
        temp_path = temp_audio.name
        content = await file.read()
        if not content:
            raise HTTPException(status_code=400, detail="Il file audio inviato e' vuoto.")
        temp_audio.write(content)

    try:
        whisper = get_whisper_model(model)
        logger.info(
            f"Trascrizione in corso per '{file.filename}' ({len(content)} bytes), "
            f"lingua: {clean_language or 'auto-detect'}, modello: {model or default_model_name}..."
        )

        transcribe_kwargs = {}
        if clean_language:
            transcribe_kwargs["language"] = clean_language
        if temperature is not None:
            transcribe_kwargs["temperature"] = temperature
        if prompt:
            transcribe_kwargs["initial_prompt"] = prompt

        def _run_transcription(whisper_model):
            segs, info = whisper_model.transcribe(temp_path, **transcribe_kwargs)
            transcript_parts = []
            segments_data: List[dict] = []
            for seg in segs:
                text = seg.text.strip()
                if text:
                    transcript_parts.append(text)
                    if include_segments:
                        segments_data.append({
                            "start": round(seg.start, 3),
                            "end": round(seg.end, 3),
                            # Convert to ms for easier Rust-side arithmetic
                            "start_ms": int(seg.start * 1000),
                            "end_ms": int(seg.end * 1000),
                            "text": text,
                            "avg_logprob": round(float(getattr(seg, "avg_logprob", 0.0)), 4),
                        })
            return transcript_parts, segments_data, info

        try:
            transcript_parts, segments_data, info = _run_transcription(whisper)
        except Exception as run_err:
            err_str = str(run_err).lower()
            if "cublas" in err_str or "cuda" in err_str or "cudnn" in err_str or "out of memory" in err_str:
                logger.warning(
                    f"Rilevato problema GPU/CUDA durante l'elaborazione ({run_err}). "
                    f"Eseguo fallback trasparente su CPU (compute_type=int8)..."
                )
                whisper_cpu = WhisperModel(model or default_model_name, device="cpu", compute_type="int8")
                models_cache[model or default_model_name] = whisper_cpu
                transcript_parts, segments_data, info = _run_transcription(whisper_cpu)
            else:
                raise run_err

        full_text = " ".join(transcript_parts)
        detected_lang = getattr(info, "language", clean_language or "unknown")
        detected_prob = getattr(info, "language_probability", 1.0)
        logger.info(f"Trascrizione completata ({len(full_text)} caratteri, lingua rilevata={detected_lang} prob={detected_prob:.2f})")

        result = {
            "text": full_text,
            "language": detected_lang,
            "duration": getattr(info, "duration", 0),
        }
        if include_segments:
            result["segments"] = segments_data

        print(result["segments"])
        return result

    except Exception as e:
        logger.error(f"Errore durante la trascrizione: {e}", exc_info=True)
        raise HTTPException(status_code=500, detail=f"Errore durante la trascrizione audio: {str(e)}")
    finally:
        if os.path.exists(temp_path):
            try:
                os.remove(temp_path)
            except Exception:
                pass


# Endpoint OpenAI standard: /v1/audio/transcriptions
@app.post("/v1/audio/transcriptions")
async def transcribe_v1(
    file: UploadFile = File(...),
    model: Optional[str] = Form(None),
    language: Optional[str] = Form(None),
    temperature: Optional[float] = Form(None),
    prompt: Optional[str] = Form(None),
):
    return await handle_transcription(file, model, language, temperature, prompt, include_segments=True)


# Endpoint OpenAI alternativo: /audio/transcriptions
@app.post("/audio/transcriptions")
async def transcribe_direct(
    file: UploadFile = File(...),
    model: Optional[str] = Form(None),
    language: Optional[str] = Form(None),
    temperature: Optional[float] = Form(None),
    prompt: Optional[str] = Form(None),
):
    return await handle_transcription(file, model, language, temperature, prompt, include_segments=True)


# Endpoint con segmenti temporizzati: /v1/audio/transcriptions/segments
# Restituisce {"text": "...", "language": "...", "duration": ..., "segments": [{start_ms, end_ms, text, avg_logprob}, ...]}
@app.post("/v1/audio/transcriptions/segments")
async def transcribe_v1_segments(
    file: UploadFile = File(...),
    model: Optional[str] = Form(None),
    language: Optional[str] = Form(None),
    temperature: Optional[float] = Form(None),
    prompt: Optional[str] = Form(None),
):
    return await handle_transcription(file, model, language, temperature, prompt, include_segments=True)


# Endpoint con segmenti temporizzati alternativo: /audio/transcriptions/segments
@app.post("/audio/transcriptions/segments")
async def transcribe_direct_segments(
    file: UploadFile = File(...),
    model: Optional[str] = Form(None),
    language: Optional[str] = Form(None),
    temperature: Optional[float] = Form(None),
    prompt: Optional[str] = Form(None),
):
    return await handle_transcription(file, model, language, temperature, prompt, include_segments=True)


def main():
    parser = argparse.ArgumentParser(description="FlowCapture faster-whisper API server")
    parser.add_argument("--host", type=str, default="0.0.0.0", help="Host su cui ascoltare (default: 0.0.0.0)")
    parser.add_argument("--port", type=int, default=8000, help="Porta su cui ascoltare (default: 8000)")
    parser.add_argument("--model", type=str, default="small", help="Modello di default (tiny, base, small, medium, large-v2, large-v3)")
    parser.add_argument("--device", type=str, default="auto", help="Device di calcolo: auto, cuda, cpu (default: auto)")
    parser.add_argument("--compute-type", type=str, default="auto", help="Compute type: auto, float16, int8, default")
    args = parser.parse_args()

    global default_model_name, configured_device, configured_compute_type
    default_model_name = args.model
    configured_device = args.device
    configured_compute_type = args.compute_type

    device, compute = get_optimal_device_and_compute()
    print("=" * 60)
    print(" FlowCapture - Server di Trascrizione faster-whisper")
    print(f" URL Locale:      http://localhost:{args.port}")
    print(f" Endpoint API:    http://localhost:{args.port}/v1/audio/transcriptions")
    print(f" Modello:         {default_model_name}")
    print(f" Dispositivo:     {device} (compute_type: {compute})")
    print("=" * 60)

    uvicorn.run(app, host=args.host, port=args.port, log_level="info")


if __name__ == "__main__":
    main()
