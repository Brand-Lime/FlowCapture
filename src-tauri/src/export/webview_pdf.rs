//! PDF printing through the app's own WebView2 (Windows only).
//!
//! Printing with an external `msedge.exe --headless --print-to-pdf` depends on how Edge is
//! installed and configured: managed/corporate profiles, startup-page policies or a running
//! instance can make it print its start page instead of the document. WebView2 is the engine
//! the app already runs on, so loading the exported page in a hidden window and calling
//! `PrintToPdf` avoids all of that.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder};
use uuid::Uuid;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2Environment6, ICoreWebView2_7, COREWEBVIEW2_PRINT_ORIENTATION_PORTRAIT,
};
use webview2_com::PrintToPdfCompletedHandler;
use windows::core::{Interface, HSTRING};

const LOAD_TIMEOUT: Duration = Duration::from_secs(60);
const PRINT_TIMEOUT: Duration = Duration::from_secs(180);
/// Time for web fonts and the embedded screenshots to finish decoding after `load`.
const SETTLE_DELAY: Duration = Duration::from_millis(600);

/// Prints `html_path` to `pdf_path`. Must not run on the main thread: the window is created
/// and printed there, and this function waits for both.
pub fn print_html_to_pdf(app: &AppHandle, html_path: &Path, pdf_path: &Path, page_size: &str) -> Result<()> {
    let url = tauri::Url::parse(&super::path_to_file_url(&html_path.to_string_lossy())?)
        .context("indirizzo del documento non valido")?;
    let label = format!("pdf-print-{}", Uuid::new_v4().simple());
    let (loaded_tx, loaded_rx) = mpsc::channel::<()>();

    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
        .title("FlowCapture PDF")
        .visible(false)
        .focused(false)
        .skip_taskbar(true)
        .inner_size(900.0, 1200.0)
        .on_page_load(move |_window, payload| {
            // Only the document itself counts, not an intermediate about:blank.
            if payload.event() == PageLoadEvent::Finished && payload.url().scheme() == "file" {
                let _ = loaded_tx.send(());
            }
        })
        .build()
        .context("impossibile creare la finestra di stampa")?;

    let result = print_loaded_window(&window, &loaded_rx, pdf_path, page_size);
    let _ = window.destroy();
    result
}

fn print_loaded_window(
    window: &tauri::WebviewWindow,
    loaded_rx: &mpsc::Receiver<()>,
    pdf_path: &Path,
    page_size: &str,
) -> Result<()> {
    loaded_rx
        .recv_timeout(LOAD_TIMEOUT)
        .map_err(|_| anyhow!("tempo scaduto durante il caricamento del documento da stampare"))?;
    std::thread::sleep(SETTLE_DELAY);

    let _ = std::fs::remove_file(pdf_path);
    let (width_in, height_in) = if page_size == "a4" { (8.27, 11.69) } else { (8.5, 11.0) };
    let target = HSTRING::from(pdf_path.as_os_str());
    let (done_tx, done_rx) = mpsc::channel::<Result<()>>();

    window
        .with_webview(move |webview| {
            let failed_tx = done_tx.clone();
            let started = (|| -> windows::core::Result<()> {
                // SAFETY: plain COM calls on interfaces owned by the live webview, made on the
                // UI thread `with_webview` runs on.
                unsafe {
                    let core = webview.controller().CoreWebView2()?;
                    let core7: ICoreWebView2_7 = core.cast()?;
                    let environment: ICoreWebView2Environment6 = webview.environment().cast()?;
                    let settings = environment.CreatePrintSettings()?;
                    settings.SetOrientation(COREWEBVIEW2_PRINT_ORIENTATION_PORTRAIT)?;
                    settings.SetPageWidth(width_in)?;
                    settings.SetPageHeight(height_in)?;
                    settings.SetMarginTop(0.0)?;
                    settings.SetMarginBottom(0.0)?;
                    settings.SetMarginLeft(0.0)?;
                    settings.SetMarginRight(0.0)?;
                    settings.SetShouldPrintBackgrounds(true)?;
                    settings.SetShouldPrintHeaderAndFooter(false)?;

                    let handler = PrintToPdfCompletedHandler::create(Box::new(move |status, succeeded| {
                        let outcome = match status {
                            Err(err) => Err(anyhow!("stampa PDF di WebView2 non riuscita: {err}")),
                            Ok(()) if !succeeded => Err(anyhow!("WebView2 non è riuscito a scrivere il PDF")),
                            Ok(()) => Ok(()),
                        };
                        let _ = done_tx.send(outcome);
                        Ok(())
                    }));
                    core7.PrintToPdf(&target, &settings, &handler)
                }
            })();
            if let Err(err) = started {
                let _ = failed_tx.send(Err(anyhow!("la stampa con WebView2 non è disponibile: {err}")));
            }
        })
        .context("impossibile raggiungere la finestra di stampa")?;

    done_rx
        .recv_timeout(PRINT_TIMEOUT)
        .map_err(|_| anyhow!("tempo scaduto durante la stampa del PDF"))??;

    if !super::is_valid_pdf(pdf_path) {
        anyhow::bail!("WebView2 non ha prodotto un PDF valido");
    }
    Ok(())
}
