# FlowCapture Companion – installazione dell'estensione browser (team IT)

FlowCapture registra i flussi di lavoro degli utenti. Nei siti web usa l'estensione **FlowCapture Companion**
per sapere su quale elemento è stato fatto clic (pulsante, campo, link). L'estensione invia questi dati solo a
FlowCapture sullo stesso computer (`http://127.0.0.1:41789`): nessun dato esce dal PC.

**FlowCapture non avvia la registrazione** se l'estensione manca o è disattivata in uno dei browser usati
sul PC (Chrome, Edge, Brave, Opera, Firefox): l'utente vede l'elenco dei browser da sistemare e
l'indicazione di contattare il team IT. I browser installati ma mai aperti dall'utente non vengono controllati.
Il controllo legge i profili dei browser, quindi l'esito si aggiorna appena l'estensione è installata
(l'utente preme "Ricontrolla").

## Pacchetti

In questa cartella:

| File | Browser | Dove si carica |
| --- | --- | --- |
| `flowcapture-companion-chromium-<versione>.zip` | Chrome, Edge, Brave, Opera | Chrome Web Store, Microsoft Edge Add-ons, Opera add-ons |
| `flowcapture-companion-firefox-<versione>.zip` | Firefox | addons.mozilla.org (firma) |

I pacchetti si rigenerano con `node scripts/build-browser-extensions.mjs` (cartella `dist-extensions/`).

## 1. Pubblicazione (una volta, poi a ogni nuova versione)

- **Chrome Web Store** (vale anche per Brave e Opera): <https://chrome.google.com/webstore/devconsole>,
  visibilità **"Non in elenco"** (installabile solo da chi ha il link). Registrazione sviluppatore: 5 $ una tantum.
- **Microsoft Edge Add-ons**: <https://partner.microsoft.com/dashboard/microsoftedge>, gratuito, visibilità nascosta.
- **Opera add-ons** (facoltativo, Opera installa anche dal Chrome Web Store): <https://addons.opera.com/developer/>.
- **Firefox**: <https://addons.mozilla.org/developers/>, scegliere **"On your own"** (non in elenco). Mozilla firma
  il pacchetto e restituisce un file `.xpi` firmato: Firefox installa solo estensioni firmate.

Ogni store assegna un **ID estensione** (32 lettere): serve per i criteri di gruppo qui sotto.

## 2. Installazione automatica sui PC (criteri di gruppo / Intune)

Installazione forzata, senza intervento dell'utente; l'estensione non può essere disattivata o rimossa.

**Google Chrome** – modelli ADMX di Chrome, criterio *"Configura l'elenco delle app e delle estensioni installate
forzatamente"* (`ExtensionInstallForcelist`), valore:

```
<ID Chrome Web Store>;https://clients2.google.com/service/update2/crx
```

**Microsoft Edge** – modelli ADMX di Edge, *"Controlla quali estensioni vengono installate automaticamente"*
(`ExtensionInstallForcelist`), valore:

```
<ID Edge Add-ons>;https://edge.microsoft.com/extensionwebstorebase/v1/crx
```

**Brave** – stesso criterio di Chrome, nella chiave `HKLM\SOFTWARE\Policies\BraveSoftware\Brave\ExtensionInstallForcelist`
(valore `1` = stringa come per Chrome).

**Firefox** – modelli ADMX di Firefox, criterio *"Extensions → Extension Management"* (`ExtensionSettings`):

```json
{
  "companion@flowcapture.app": {
    "installation_mode": "force_installed",
    "install_url": "https://addons.mozilla.org/firefox/downloads/latest/companion@flowcapture.app/latest.xpi"
  }
}
```

Con la pubblicazione "On your own" usare come `install_url` il percorso del file `.xpi` firmato su una cartella
condivisa (`file:///S:/FlowCapture/flowcapture-companion.xpi`) o su un server web interno.

**Opera** non supporta i criteri di gruppo: l'utente installa l'estensione dal link dello store (vedi sotto).

## 3. Installazione manuale (PC singolo)

- **Chrome / Edge / Brave / Opera**: aprire il link dell'estensione nello store e premere *Aggiungi*.
  (Opera: prima installare "Install Chrome Extensions" dallo store di Opera se si usa il link del Chrome Web Store.)
- **Firefox**: aprire il file `.xpi` firmato con Firefox (trascinarlo nella finestra) e confermare.
- Solo per prove: `chrome://extensions` → *Modalità sviluppatore* → *Carica estensione non pacchettizzata* →
  cartella `chromium` del pacchetto. Edge chiede a ogni avvio di disattivarla: non usarlo per gli utenti.

## 4. Eccezioni

Con FlowCapture avviato con il parametro `--admin`, in *Impostazioni → Estensione browser* si può rendere
l'estensione non obbligatoria su quel PC.
