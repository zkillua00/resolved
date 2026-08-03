//! Product identity shared by the native shell and branded UI surfaces.

pub const PRODUCT_NAME: &str = "Resolved";
pub const BRAND_HEADLINE: &str = "Know exactly what you sent.";
pub const BRAND_EXPLANATION: &str = "Build the request. Resolve its context. Inspect the result.";
// Keep the in-app image near its largest rendered Retina size. The macOS app
// icon remains the full-resolution `macos/Resolved.icns` bundle resource.
pub const ICON_ASSET_PATH: &str = "brand/resolved-runtime.png";
