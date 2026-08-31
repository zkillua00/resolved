use std::path::Path;

use gpui::{App, Menu, MenuItem, SystemMenuType, px};
use wry::WebViewBuilder;

use super::{PlatformBackend, TitleBarIntegration};
use crate::{
    brand::PRODUCT_NAME,
    shortcuts::{
        CloseRequestTab, NewRequestTab, QuitApp, SaveRequest, SaveRequestAs, SendOrCancelRequest,
        ShowSettings,
    },
};

pub(super) struct MacOsBackend;

impl PlatformBackend for MacOsBackend {
    fn launch_blocker() -> Result<(), String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("could not resolve the executable path: {error}"))?;
        if containing_app_bundle(&executable).is_some() {
            Ok(())
        } else {
            Err(
                "Resolved must run from its macOS application bundle. Use scripts/cargo.sh run."
                    .to_owned(),
            )
        }
    }

    fn configure_menus(cx: &mut App) {
        cx.set_menus(vec![
            Menu {
                name: PRODUCT_NAME.into(),
                items: vec![
                    MenuItem::action("Settings…", ShowSettings),
                    MenuItem::separator(),
                    MenuItem::os_submenu("Services", SystemMenuType::Services),
                    MenuItem::separator(),
                    MenuItem::action(format!("Quit {PRODUCT_NAME}"), QuitApp),
                ],
            },
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("New Request Tab", NewRequestTab),
                    MenuItem::action("Close Active Tab", CloseRequestTab),
                    MenuItem::separator(),
                    MenuItem::action("Save", SaveRequest),
                    MenuItem::action("Save As…", SaveRequestAs),
                ],
            },
            Menu {
                name: "Request".into(),
                items: vec![MenuItem::action(
                    "Send or Cancel Request",
                    SendOrCancelRequest,
                )],
            },
        ]);
    }

    fn title_bar_integration() -> TitleBarIntegration {
        TitleBarIntegration {
            leading_inset: px(92.),
            trailing_inset: px(24.),
            client_controls: false,
            draggable_content: false,
        }
    }

    fn configure_webview<'a>(builder: WebViewBuilder<'a>) -> WebViewBuilder<'a> {
        wry::WebViewBuilderExtDarwin::with_allow_link_preview(builder.with_incognito(true), false)
    }
}

fn containing_app_bundle(executable: &Path) -> Option<&Path> {
    let macos = executable.parent()?;
    if macos.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let bundle = contents.parent()?;
    (bundle.extension()? == "app").then_some(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_executables_inside_macos_app_bundles() {
        let bundled = Path::new("/tmp/Resolved.app/Contents/MacOS/api-tester");
        assert_eq!(
            containing_app_bundle(bundled),
            Some(Path::new("/tmp/Resolved.app"))
        );
        assert!(containing_app_bundle(Path::new("/tmp/target/debug/api-tester")).is_none());
        assert!(
            containing_app_bundle(Path::new("/tmp/Resolved/Contents/MacOS/api-tester")).is_none()
        );
    }
}
