use std::{
    io::Write as _,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use gpui::{App, Keystroke, px};
use windows::Win32::{
    Foundation::{ERROR_INSUFFICIENT_BUFFER, WIN32_ERROR},
    Storage::Packaging::Appx::GetCurrentPackageFullName,
};
use wry::WebView;

use super::{PlatformBackend, TitleBarIntegration, configure_desktop_menus, control_shortcut};
use crate::core::DatabaseStore;

pub(super) struct WindowsBackend;

impl PlatformBackend for WindowsBackend {
    fn launch_blocker() -> Result<(), String> {
        if has_package_identity() {
            Ok(())
        } else {
            Err(
                "Resolved must run as a packaged (MSIX) application on Windows. Use \
                 scripts/cargo.ps1 run to build and launch the packaged app."
                    .to_owned(),
            )
        }
    }

    fn configure_menus(cx: &mut App) {
        configure_desktop_menus(cx);
    }

    fn install_runtime_hooks() {
        std::panic::set_hook(Box::new(|info| {
            let path = diagnostic_log_path();
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                let _ = writeln!(
                    file,
                    "{}\n{info}\nbacktrace:\n{}",
                    chrono::Local::now().to_rfc3339(),
                    std::backtrace::Backtrace::force_capture()
                );
            }
            eprintln!("{info}");
        }));

        // GPUI's DirectComposition visual otherwise covers native child HWNDs
        // such as the WebView2 preview. This runs before GPUI is initialized.
        // SAFETY: startup is still single-threaded and no environment readers
        // have been created by the application.
        unsafe {
            std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "1");
        }
    }

    fn log_diagnostic(message: &str) {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(diagnostic_log_path())
        {
            let _ = writeln!(file, "{} {message}", chrono::Local::now().to_rfc3339());
        }
    }

    fn normalize_keystroke(keystroke: &mut Keystroke) {
        control_shortcut(keystroke);
    }

    fn title_bar_integration() -> TitleBarIntegration {
        TitleBarIntegration {
            leading_inset: px(12.),
            trailing_inset: px(0.),
            client_controls: true,
            draggable_content: true,
        }
    }

    fn load_preview_document(
        webview: &WebView,
        document: &str,
    ) -> wry::Result<Option<Arc<AtomicBool>>> {
        let encoded =
            serde_json::to_string(document).expect("serializing an HTML string cannot fail");
        let ready = Arc::new(AtomicBool::new(false));
        let callback_ready = Arc::clone(&ready);
        webview.evaluate_script_with_callback(
            &format!("document.open();document.write({encoded});document.close();true"),
            move |_| callback_ready.store(true, Ordering::Release),
        )?;
        Ok(Some(ready))
    }

    fn preview_data_directory() -> Option<PathBuf> {
        Some(
            DatabaseStore::default_path()
                .with_file_name("webview-data")
                .parent()
                .map(|directory| directory.join("webview-data"))
                .unwrap_or_else(|| std::env::temp_dir().join("resolved-webview-data")),
        )
    }
}

fn has_package_identity() -> bool {
    let mut length: u32 = 0;
    // SAFETY: `length` is a valid writable u32 for the size probe; no name
    // buffer is needed to distinguish a packaged process.
    let status: WIN32_ERROR = unsafe { GetCurrentPackageFullName(&mut length, None) };
    status == WIN32_ERROR(0) || status == ERROR_INSUFFICIENT_BUFFER
}

fn diagnostic_log_path() -> PathBuf {
    DatabaseStore::default_path().with_file_name("api-tester-panic.log")
}
