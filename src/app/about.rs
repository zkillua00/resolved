use gpui_component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use semver::Version;
use serde::Deserialize;

use super::*;

const DOWNLOADS_URL: &str = "https://apiworkbench.dev/downloads.json";

#[derive(Default)]
pub(super) enum UpdateStatus {
    #[default]
    Unchecked,
    Checking,
    Checked(ReleaseCheck),
    Failed(String),
}

#[derive(Deserialize)]
struct Downloads {
    version: String,
    resolved: HashMap<String, HashMap<String, Vec<ReleaseDownload>>>,
}

#[derive(Clone, Debug, Deserialize)]
struct ReleaseDownload {
    name: String,
    url: String,
}

pub(super) struct ReleaseCheck {
    version: Version,
    newer: bool,
    downloads: Vec<ReleaseDownload>,
}

impl Downloads {
    fn check(self, installed: &str, os: &str, arch: &str) -> Result<ReleaseCheck, String> {
        let version = Version::parse(&self.version)
            .map_err(|_| "The download feed contains an invalid version.".to_owned())?;
        // Nightlies use MAJOR.MINOR.PATCH.<12-character-commit-hash>, not SemVer.
        // Compare their base version against the stable release feed.
        let installed = Version::parse(installed)
            .or_else(|error| {
                let Some((base, hash)) = installed.rsplit_once('.') else {
                    return Err(error);
                };
                if hash.len() == 12 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    Version::parse(base)
                } else {
                    Err(error)
                }
            })
            .map_err(|_| "The installed build has an invalid version.".to_owned())?;
        let arch = match arch {
            "aarch64" => "arm64",
            "x86_64" => "x64",
            other => other,
        };
        let downloads = self
            .resolved
            .get(os)
            .and_then(|platform| platform.get(arch))
            .cloned()
            .unwrap_or_default();
        // The feed supplies release links, never commands or arbitrary URL schemes.
        for download in &downloads {
            let valid = url::Url::parse(&download.url).is_ok_and(|url| {
                url.scheme() == "https"
                    && url.host_str() == Some("github.com")
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.port_or_known_default() == Some(443)
                    && url
                        .path()
                        .starts_with("/zkillua00/resolved/releases/download/")
            });
            if !valid {
                return Err("The download feed contains an invalid GitHub release link.".to_owned());
            }
        }
        Ok(ReleaseCheck {
            newer: version.cmp_precedence(&installed).is_gt(),
            version,
            downloads,
        })
    }
}

async fn check_for_updates() -> Result<ReleaseCheck, String> {
    // Keep this separate from request/workspace clients: no user cookies,
    // credentials, custom certificate settings, or request history.
    crate::tls::install_crypto_provider().map_err(str::to_owned)?;
    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("resolved/", env!("RESOLVED_BUILD_VERSION")))
        .build()
        .map_err(|error| error.to_string())?;
    let feed = client
        .get(DOWNLOADS_URL)
        .header(reqwest::header::CACHE_CONTROL, "no-cache")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| error.to_string())?
        .json::<Downloads>()
        .await
        .map_err(|error| format!("Could not read the download feed: {error}"))?;
    feed.check(
        env!("RESOLVED_BUILD_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

impl ApiTester {
    pub(crate) fn on_show_about_resolved(
        &mut self,
        _: &shortcuts::ShowAboutResolved,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.about_page_request = self.about_page_request.wrapping_add(1).max(1);
        self.open_workspace_tool_tab(WorkspaceToolTab::Settings, window, cx);
    }

    pub(crate) fn on_check_for_updates(
        &mut self,
        _: &shortcuts::CheckForUpdates,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_show_about_resolved(&shortcuts::ShowAboutResolved, window, cx);
        self.start_update_check(cx);
    }

    pub(super) fn about_settings_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let this = cx.entity().downgrade();
        SettingPage::new("About Resolved")
            .description("Version, publisher, and release updates.")
            .resettable(false)
            .group(
                SettingGroup::new().title("Resolved").items([
                    SettingItem::new(
                        "Version",
                        SettingField::<SharedString>::render(|_, _, _| {
                            div()
                                .debug_selector(|| "about-version".to_owned())
                                .text_sm()
                                .child(env!("RESOLVED_BUILD_VERSION"))
                        }),
                    ),
                    SettingItem::new(
                        "Publisher",
                        SettingField::<SharedString>::render(|_, _, _| {
                            div()
                                .debug_selector(|| "about-publisher".to_owned())
                                .child(
                                    Button::new("about-publisher")
                                        .label("apiworkbench.dev")
                                        .link()
                                        .on_click(|_, _, cx| cx.open_url("https://apiworkbench.dev")),
                                )
                        }),
                    ),
                    SettingItem::new(
                        "Build hash (Git commit)",
                        SettingField::<SharedString>::render(|_, _, _| {
                            h_flex()
                                .gap_2()
                                .min_w_0()
                                .pb_px()
                                .child(
                                    div()
                                        .min_w_0()
                                        .text_sm()
                                        .truncate()
                                        .child(env!("RESOLVED_BUILD_COMMIT")),
                                )
                                .child(
                                    Button::new("about-copy-commit")
                                        .debug_selector(|| "about-copy-commit".to_owned())
                                        .label("Copy")
                                        .outline()
                                        .small()
                                        .tooltip("Copy build commit hash")
                                        .disabled(env!("RESOLVED_BUILD_COMMIT") == "Unknown")
                                        .on_click(|_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                env!("RESOLVED_BUILD_COMMIT").to_owned(),
                                            ));
                                        }),
                                )
                        }),
                    ),
                ]),
            )
            .group(
                SettingGroup::new()
                    .title("Updates")
                    .description("Checks apiworkbench.dev for GitHub releases. Downloads open in your browser.")
                    .item(
                        SettingItem::new(
                            "Available updates",
                            SettingField::<SharedString>::render(move |_, _, cx| {
                                let Some(entity) = this.upgrade() else {
                                    return div().into_any_element();
                                };
                                let state = entity.read(cx);
                                let checking = matches!(state.update_status, UpdateStatus::Checking);
                                let message = match &state.update_status {
                                    UpdateStatus::Unchecked => "Not checked yet.".to_owned(),
                                    UpdateStatus::Checking => "Checking for updates…".to_owned(),
                                    UpdateStatus::Checked(release) if release.newer => {
                                        format!("Resolved {} is available.", release.version)
                                    }
                                    UpdateStatus::Checked(release) => {
                                        format!("No newer release available (latest: {}).", release.version)
                                    }
                                    UpdateStatus::Failed(error) => {
                                        format!("Could not check for updates: {error}")
                                    }
                                };
                                let mut content = v_flex()
                                    .debug_selector(|| "about-updates-content".to_owned())
                                    // Keep the outline inside the Settings field's overflow
                                    // clip when nested layout rounds fractional pixel heights.
                                    .pb_px()
                                    .gap_2()
                                    .child(div().text_sm().child(message));
                                if let UpdateStatus::Checked(release) = &state.update_status
                                    && release.newer
                                {
                                    if release.downloads.is_empty() {
                                        content = content.child(
                                            div().text_sm().child("No download is listed for this OS and architecture."),
                                        );
                                    }
                                    for (index, download) in release.downloads.iter().enumerate() {
                                        let url = download.url.clone();
                                        content = content.child(
                                            h_flex()
                                                .debug_selector(move || format!("about-download-{index}"))
                                                .child(
                                                    Button::new(("about-download", index))
                                                        .label(download.name.clone())
                                                        .link()
                                                        .on_click(move |_, _, cx| cx.open_url(&url)),
                                                ),
                                        );
                                    }
                                }
                                let this = this.clone();
                                content
                                    .child(
                                        h_flex()
                                            .debug_selector(|| "about-check-updates".to_owned())
                                            .child(
                                                Button::new("about-check-updates")
                                                    .debug_selector(|| "about-check-updates-button".to_owned())
                                                    .label("Check for updates")
                                                    .outline()
                                                    .disabled(checking)
                                                    .on_click(move |_, _, cx| {
                                                        if let Some(this) = this.upgrade() {
                                                            this.update(cx, |this, cx| this.start_update_check(cx));
                                                        }
                                                    }),
                                            ),
                                    )
                                    .into_any_element()
                            }),
                        )
                        .layout(gpui::Axis::Vertical),
                    ),
            )
    }

    fn start_update_check(&mut self, cx: &mut Context<Self>) {
        if matches!(self.update_status, UpdateStatus::Checking) {
            return;
        }
        self.update_status = UpdateStatus::Checking;
        let task = self.runtime.spawn(check_for_updates());
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|error| Err(error.to_string()));
            let _ = this.update(cx, |this, cx| {
                this.update_status = match result {
                    Ok(release) => UpdateStatus::Checked(release),
                    Err(error) => UpdateStatus::Failed(error),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Modifiers, TestAppContext, size};

    use super::*;

    fn feed(version: &str) -> Downloads {
        serde_json::from_value(serde_json::json!({
            "version": version,
            "resolved": {
                "macos": {
                    "arm64": [{
                        "name": "Resolved-macos-arm64.zip",
                        "url": "https://github.com/zkillua00/resolved/releases/download/v0.12.2/Resolved-macos-arm64.zip",
                        "size": 20750748,
                        "sha256": "archive-checksum"
                    }],
                    "x64": [{
                        "name": "Resolved-macos-x64.zip",
                        "url": "https://github.com/zkillua00/resolved/releases/download/v0.12.2/Resolved-macos-x64.zip"
                    }]
                },
                "windows": {
                    "x64": [
                        {"name": "Resolved.cer", "url": "https://github.com/zkillua00/resolved/releases/download/v0.12.2/Resolved.cer"},
                        {"name": "Resolved.msix", "url": "https://github.com/zkillua00/resolved/releases/download/v0.12.2/Resolved.msix"}
                    ]
                },
                "linux": {
                    "arm64": [
                        {"name": "Resolved.rpm", "url": "https://github.com/zkillua00/resolved/releases/download/v0.12.2/Resolved.rpm"},
                        {"name": "Resolved.tar.xz", "url": "https://github.com/zkillua00/resolved/releases/download/v0.12.2/Resolved.tar.xz"},
                        {"name": "Resolved.deb", "url": "https://github.com/zkillua00/resolved/releases/download/v0.12.2/Resolved.deb"}
                    ]
                }
            },
            "resolved-mcp": {},
            "additional_files": []
        })).unwrap()
    }

    #[test]
    fn compares_versions_numerically_including_nightly_builds() {
        for (latest, installed, newer) in [
            ("0.12.2", "0.9.9", true),
            ("0.12.2", "0.12.2", false),
            ("0.12.2", "0.13.0", false),
            ("0.12.2", "0.12.2.abcdef123456", false),
            ("0.12.3", "0.12.2.abcdef123456", true),
            ("0.12.2", "0.12.2-rc.1", true),
            ("0.12.2+build.2", "0.12.2+build.1", false),
        ] {
            assert_eq!(
                feed(latest)
                    .check(installed, "macos", "aarch64")
                    .unwrap()
                    .newer,
                newer,
                "{latest} vs {installed}"
            );
        }
    }

    #[test]
    fn selects_only_desktop_downloads_for_the_current_platform() {
        for (os, arch, expected) in [
            ("macos", "aarch64", vec!["Resolved-macos-arm64.zip"]),
            ("macos", "x86_64", vec!["Resolved-macos-x64.zip"]),
            ("windows", "x86_64", vec!["Resolved.cer", "Resolved.msix"]),
            ("windows", "aarch64", vec![]),
            (
                "linux",
                "aarch64",
                vec!["Resolved.rpm", "Resolved.tar.xz", "Resolved.deb"],
            ),
        ] {
            let release = feed("0.12.2").check("0.12.1", os, arch).unwrap();
            assert_eq!(
                release
                    .downloads
                    .iter()
                    .map(|download| download.name.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[test]
    fn rejects_invalid_versions_and_release_links() {
        assert!(
            feed("not-a-version")
                .check("0.12.2", "macos", "aarch64")
                .is_err()
        );
        assert!(
            feed("0.12.2")
                .check("0.12.2.invalid", "macos", "aarch64")
                .is_err()
        );
        assert!(serde_json::from_str::<Downloads>(r#"{"version":"0.12.2"}"#).is_err());
        for url in [
            "file:///tmp/download",
            "http://github.com/zkillua00/resolved/releases/download/v0.12.2/app.zip",
            "https://github.com.evil.example/zkillua00/resolved/releases/download/app.zip",
            "https://github.com/someone/else/releases/download/app.zip",
        ] {
            let mut feed = feed("0.12.2");
            feed.resolved
                .get_mut("macos")
                .unwrap()
                .get_mut("arm64")
                .unwrap()[0]
                .url = url.to_owned();
            assert!(feed.check("0.12.1", "macos", "aarch64").is_err());
        }
    }

    struct SettingsHarness(Entity<ApiTester>);

    impl Render for SettingsHarness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            gpui_component::setting::Settings::new("about-settings-test")
                .with_group_variant(gpui_component::group_box::GroupBoxVariant::Outline)
                .pages([self.0.update(cx, |app, cx| app.about_settings_page(cx))])
        }
    }

    #[gpui::test]
    fn about_page_renders_and_links_and_copy_work(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        store.initialize().unwrap();
        let release = feed("0.12.2").check("0.12.1", "macos", "aarch64").unwrap();
        let download_url = release.downloads[0].url.clone();
        let mut about_app = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            let bindings = shortcuts::capture_base_key_bindings(cx);
            crate::theme::configure(cx);
            let app = cx.new(|cx| {
                let mut app = ApiTester::new_with_database_store(bindings, store, window, cx);
                app.update_status = UpdateStatus::Checked(release);
                app
            });
            about_app = Some(app.clone());
            let view = cx.new(|_| SettingsHarness(app));
            Root::new(view, window, cx)
        });
        cx.simulate_resize(size(px(900.), px(800.)));
        cx.update(|window, _| {
            window.activate_window();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("about-version").is_some());
        assert!(cx.debug_bounds("about-check-updates").is_some());
        let content = cx.debug_bounds("about-updates-content").unwrap();
        let button = cx.debug_bounds("about-check-updates-button").unwrap();
        let row = cx.debug_bounds("about-check-updates").unwrap();
        assert!(
            button.is_contained_within(&content) && button.is_contained_within(&row),
            "button {button:?} must fit its row {row:?} and field {content:?}"
        );

        let publisher = cx.debug_bounds("about-publisher").unwrap();
        cx.simulate_click(publisher.center(), Modifiers::none());
        assert_eq!(cx.opened_url().as_deref(), Some("https://apiworkbench.dev"));

        if env!("RESOLVED_BUILD_COMMIT") != "Unknown" {
            let copy = cx.debug_bounds("about-copy-commit").unwrap();
            cx.simulate_click(copy.center(), Modifiers::none());
            assert_eq!(
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .as_deref(),
                Some(env!("RESOLVED_BUILD_COMMIT"))
            );
        }

        let download = cx.debug_bounds("about-download-0").unwrap();
        cx.simulate_click(
            point(download.left() + px(10.), download.center().y),
            Modifiers::none(),
        );
        assert_eq!(cx.opened_url().as_deref(), Some(download_url.as_str()));
        let about_app = about_app.unwrap();
        for zoom in [0.8, 0.9, 1.0, 1.1, 1.25] {
            cx.update(|window, cx| {
                about_app.update(cx, |app, cx| {
                    app.update_status = UpdateStatus::Checked(
                        feed("0.12.2").check("0.12.3", "macos", "aarch64").unwrap(),
                    );
                    crate::theme::set_zoom(
                        crate::theme::ThemeZoom {
                            ui: zoom,
                            editor: 1.,
                        },
                        cx,
                    );
                    crate::theme::configure(cx);
                    cx.notify();
                });
                window.refresh();
            });
            cx.run_until_parked();
            let content = cx.debug_bounds("about-updates-content").unwrap();
            let button = cx.debug_bounds("about-check-updates-button").unwrap();
            let row = cx.debug_bounds("about-check-updates").unwrap();
            assert!(
                button.is_contained_within(&content) && button.is_contained_within(&row),
                "zoom {zoom}: button {button:?} must fit its row {row:?} and field {content:?}"
            );
        }
    }

    #[gpui::test]
    fn menu_actions_open_about_clear_search_and_check_only_when_requested(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        store.initialize().unwrap();
        let mut app = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            let bindings = shortcuts::capture_base_key_bindings(cx);
            crate::theme::configure(cx);
            let view = cx.new(|cx| {
                let mut app = ApiTester::new_with_database_store(bindings, store, window, cx);
                // Leave this runtime undriven so the menu test observes the queued
                // check deterministically without contacting the public feed.
                app.runtime = Arc::new(
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap(),
                );
                app
            });
            crate::register_app_action_handlers(&view, cx);
            app = Some(view.clone());
            Root::new(view, window, cx)
        });
        let app = app.unwrap();
        cx.simulate_resize(size(px(1200.), px(900.)));
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();

        cx.dispatch_action(shortcuts::ShowSettings);
        cx.run_until_parked();
        assert!(cx.debug_bounds("upstream-settings-list").is_some());
        cx.dispatch_action(shortcuts::ShowAboutResolved);
        cx.run_until_parked();
        assert!(cx.debug_bounds("about-version").is_some());
        cx.update(|_, cx| {
            let app = app.read(cx);
            assert_eq!(app.workspace_tabs.active(), ActiveWorkspaceTab::Settings);
            assert!(matches!(app.update_status, UpdateStatus::Unchecked));
        });

        let search_bounds = cx.debug_bounds("settings-search").unwrap();
        cx.simulate_click(search_bounds.center(), Modifiers::none());
        cx.run_until_parked();
        let search = cx.update(|window, cx| window.focused_input(cx)).unwrap();
        cx.update(|window, cx| {
            search.update(cx, |input, cx| input.set_value("Tab size", window, cx));
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(search.read(cx).value().as_ref(), "Tab size");
            app.update(cx, |_, cx| cx.notify());
        });
        cx.run_until_parked();
        cx.update(|_, cx| assert_eq!(search.read(cx).value().as_ref(), "Tab size"));

        cx.dispatch_action(shortcuts::ShowAboutResolved);
        cx.run_until_parked();
        assert!(cx.debug_bounds("about-version").is_some());
        cx.update(|_, cx| {
            assert!(search.read(cx).value().is_empty());
            assert!(matches!(
                app.read(cx).update_status,
                UpdateStatus::Unchecked
            ));
        });

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.activate_request_workspace(SidebarTab::Collections, window, cx);
            });
        });
        cx.run_until_parked();
        cx.dispatch_action(shortcuts::CheckForUpdates);
        cx.run_until_parked();
        assert!(cx.debug_bounds("about-version").is_some());
        cx.update(|_, cx| {
            let app = app.read(cx);
            assert_eq!(app.workspace_tabs.active(), ActiveWorkspaceTab::Settings);
            assert!(matches!(app.update_status, UpdateStatus::Checking));
        });
    }

    #[tokio::test]
    #[ignore = "manual smoke test against the public download feed"]
    async fn live_download_feed() {
        let release = check_for_updates().await.expect("check live download feed");
        assert!(!release.downloads.is_empty());
    }
}
