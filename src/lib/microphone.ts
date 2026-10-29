/** Problem with the microphone, with a message the user can act on. */
export class MicrophoneError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "MicrophoneError";
  }
}

export function microphoneConstraints(deviceId: string): MediaStreamConstraints {
  return { audio: deviceId ? { deviceId: { exact: deviceId } } : true };
}

/** Turns a getUserMedia failure into an explanation for a non technical user. */
export function describeMicrophoneError(err: unknown): string {
  const name = err instanceof DOMException || err instanceof Error ? err.name : "";
  switch (name) {
    case "NotFoundError":
    case "DevicesNotFoundError":
      return "Nessun microfono trovato. Collega un microfono (o una cuffia con microfono) e premi \"Aggiorna lista\".";
    case "OverconstrainedError":
      return "Il microfono selezionato non è più collegato. Scegline un altro dalla lista oppure ricollegalo.";
    case "NotAllowedError":
    case "SecurityError":
      return "Windows non permette a FlowCapture di usare il microfono. Apri Impostazioni di Windows → Privacy e sicurezza → Microfono e attiva \"Consenti alle app desktop di accedere al microfono\".";
    case "NotReadableError":
    case "AbortError":
      return "Il microfono è occupato da un'altra applicazione (Teams, Zoom, ...) o non risponde. Chiudi l'altra applicazione e riprova.";
    default:
      return `Il microfono non funziona (${err instanceof Error ? err.message : String(err)}).`;
  }
}

/** Opens the microphone, or throws a MicrophoneError explaining what is wrong. */
export async function openMicrophone(deviceId: string): Promise<MediaStream> {
  if (!navigator?.mediaDevices?.getUserMedia) {
    throw new MicrophoneError("Questo sistema non permette di registrare l'audio.");
  }
  try {
    return await navigator.mediaDevices.getUserMedia(microphoneConstraints(deviceId));
  } catch (err) {
    throw new MicrophoneError(describeMicrophoneError(err));
  }
}

/** Best audio format the WebView can record. */
export function pickAudioMimeType(): string {
  for (const type of ["audio/webm;codecs=opus", "audio/ogg;codecs=opus", "audio/mp4"]) {
    if (MediaRecorder.isTypeSupported(type)) return type;
  }
  return "audio/webm";
}
