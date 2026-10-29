# FlowCapture Companion Extension

Estensione per Google Chrome, Microsoft Edge, Brave, Opera e Mozilla Firefox.

I pacchetti per gli store (Chromium e Firefox) si creano con `node scripts/build-browser-extensions.mjs`
(cartella `dist-extensions/`); come pubblicarla e installarla sui PC con i criteri di gruppo è descritto in
`docs/estensione-browser-IT.md`. FlowCapture non avvia la registrazione se l'estensione manca in un browser usato.
Cattura con precisione l'elemento cliccato (testo, tag, tipo, selettore CSS, URL) e lo invia direttamente a **FlowCapture** via bridge locale (`127.0.0.1:41789`).

---

## Come installarla in Google Chrome / Microsoft Edge

1. Apri Google Chrome e naviga su `chrome://extensions` (o `edge://extensions` in Edge).
2. In alto a destra, attiva l'interruttore **Modalità sviluppatore** (Developer mode).
3. Fai clic sul pulsante **Carica estensione non pacchettizzata** (Load unpacked).
4. Seleziona questa cartella (`c:\Pro\FlowCapture\browser-extension`).
5. L'icona di **FlowCapture** apparirà nella barra delle estensioni del browser.

---

## Come funziona

- Quando FlowCapture è in esecuzione, il bridge locale ascolta sulla porta `41789`.
- L'estensione rileva automaticamente i click dell'utente sulle pagine web.
- Estrae il testo semantico dell'elemento (`aria-label`, testo del pulsante, placeholder, link), il tag (`button`, `a`, `input`), l'URL della pagina e il selettore CSS.
- Invia istantaneamente i metadati a FlowCapture, che li combina con lo screenshot e l'evento del mouse.
- L'AI di FlowCapture utilizzerà queste informazioni per generare passaggi precisi come *"Fai clic su 'Accedi' in Google"* invece di riferirsi solo al nome della pagina.
