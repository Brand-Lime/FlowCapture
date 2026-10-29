import { useEffect, useRef, useState } from "react";
import { AppButton } from "@/components/ui/AppButton";
import { MicrophoneError, openMicrophone, pickAudioMimeType } from "@/lib/microphone";

/** How long the test records. */
const TEST_SECONDS = 5;
/** Peak level (0-1) under which the test counts as "nothing heard". */
const SILENCE_PEAK = 0.04;

type TestState =
  | { kind: "idle" }
  | { kind: "listening"; secondsLeft: number }
  | { kind: "ok"; audioUrl: string | null }
  | { kind: "silent"; audioUrl: string | null }
  | { kind: "error"; message: string };

/**
 * "Prova microfono": records a few seconds from the chosen microphone with a live level meter,
 * then tells whether a voice was heard and lets the user listen to it.
 */
export function MicrophoneTest({ deviceId, onDevicesChanged }: { deviceId: string; onDevicesChanged?: () => void }) {
  const [state, setState] = useState<TestState>({ kind: "idle" });
  const [level, setLevel] = useState(0);
  const stopRef = useRef<(() => void) | null>(null);
  const audioUrlRef = useRef<string | null>(null);

  useEffect(
    () => () => {
      stopRef.current?.();
      if (audioUrlRef.current) URL.revokeObjectURL(audioUrlRef.current);
    },
    [],
  );

  // A test made with another microphone says nothing about the new one.
  useEffect(() => {
    stopRef.current?.();
    setState({ kind: "idle" });
  }, [deviceId]);

  async function runTest() {
    stopRef.current?.();
    if (audioUrlRef.current) {
      URL.revokeObjectURL(audioUrlRef.current);
      audioUrlRef.current = null;
    }
    setLevel(0);

    let stream: MediaStream;
    try {
      stream = await openMicrophone(deviceId);
    } catch (err) {
      setState({ kind: "error", message: err instanceof MicrophoneError ? err.message : String(err) });
      return;
    }
    // Labels are only visible once access was granted.
    onDevicesChanged?.();

    const context = new AudioContext();
    const analyser = context.createAnalyser();
    analyser.fftSize = 1024;
    context.createMediaStreamSource(stream).connect(analyser);
    void context.resume().catch(() => undefined);
    const samples = new Float32Array(analyser.fftSize);

    const chunks: Blob[] = [];
    const mimeType = pickAudioMimeType();
    const recorder = new MediaRecorder(stream, { mimeType });
    recorder.ondataavailable = (event) => {
      if (event.data.size > 0) chunks.push(event.data);
    };

    let peak = 0;
    let frame = 0;
    let finished = false;
    const startedAt = Date.now();

    const cleanup = () => {
      finished = true;
      cancelAnimationFrame(frame);
      window.clearInterval(countdown);
      stream.getTracks().forEach((track) => track.stop());
      void context.close().catch(() => undefined);
      stopRef.current = null;
    };

    const tick = () => {
      if (finished) return;
      analyser.getFloatTimeDomainData(samples);
      let sum = 0;
      for (const value of samples) sum += value * value;
      const rms = Math.sqrt(sum / samples.length);
      // Speech sits around 0.02-0.2 RMS: scale it so a normal voice fills most of the bar.
      const scaled = Math.min(1, rms * 6);
      peak = Math.max(peak, scaled);
      setLevel(scaled);
      frame = requestAnimationFrame(tick);
    };

    const countdown = window.setInterval(() => {
      const left = TEST_SECONDS - Math.floor((Date.now() - startedAt) / 1000);
      if (left > 0) {
        setState({ kind: "listening", secondsLeft: left });
      } else if (recorder.state !== "inactive") {
        recorder.stop();
      }
    }, 250);

    recorder.onstop = () => {
      const cancelled = finished;
      cleanup();
      setLevel(0);
      if (cancelled) return;
      const audioUrl = chunks.length > 0 ? URL.createObjectURL(new Blob(chunks, { type: mimeType })) : null;
      audioUrlRef.current = audioUrl;
      setState(peak >= SILENCE_PEAK ? { kind: "ok", audioUrl } : { kind: "silent", audioUrl });
    };

    stopRef.current = () => {
      cleanup();
      if (recorder.state !== "inactive") recorder.stop();
      setLevel(0);
      setState({ kind: "idle" });
    };

    recorder.start();
    setState({ kind: "listening", secondsLeft: TEST_SECONDS });
    frame = requestAnimationFrame(tick);
  }

  const listening = state.kind === "listening";

  return (
    <div className="mic-test">
      <div className="mic-test-row">
        <AppButton
          size="sm"
          icon="mic"
          onClick={() => (listening ? stopRef.current?.() : void runTest())}
          title="Registra qualche secondo dal microfono scelto per controllare che funzioni"
        >
          {listening ? "Interrompi prova" : "Prova microfono"}
        </AppButton>
        {listening ? (
          <>
            <div className="mic-meter" aria-label="Livello del microfono">
              <i style={{ width: `${Math.round(level * 100)}%` }} />
            </div>
            <span className="mic-test-hint">Parla ora… {state.secondsLeft}s</span>
          </>
        ) : null}
      </div>

      {state.kind === "ok" ? (
        <div className="mic-test-result ok">
          ✅ Il microfono funziona: la tua voce è stata sentita.
          {state.audioUrl ? <audio controls src={state.audioUrl} /> : null}
        </div>
      ) : null}
      {state.kind === "silent" ? (
        <div className="mic-test-result warn">
          ⚠️ Non è stato sentito nessun suono. Controlla che il microfono non sia spento o in muto (tasto sulla
          cuffia), che sia quello giusto nella lista e avvicinati mentre parli.
          {state.audioUrl ? <audio controls src={state.audioUrl} /> : null}
        </div>
      ) : null}
      {state.kind === "error" ? <div className="mic-test-result error">❌ {state.message}</div> : null}
    </div>
  );
}
