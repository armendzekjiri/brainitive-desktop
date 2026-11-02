//! The MSIX manifest template of the Microsoft Store build (BRAI-442) against the identity
//! Partner Center reserved (docs/MICROSOFT-STORE-LISTING.md § 7) and the constants the app uses
//! at runtime (`msstore`). The template is rendered by scripts/msix-manifest.mjs (tested by
//! scripts/msix-manifest.test.mjs); the version rule is checked here as well.

use brainitive_desktop_lib::msstore;

fn read(path: &str) -> String {
    std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn template() -> String {
    read("msix/AppxManifest.xml.tmpl")
}

/// `attribute="value"` of the first `<tag …>` in the template.
fn attr(xml: &str, tag: &str, attribute: &str) -> String {
    let start = xml.find(&format!("<{tag} ")).or_else(|| xml.find(&format!("<{tag}\n")));
    let start = start.unwrap_or_else(|| panic!("no <{tag}> in the template"));
    let element = &xml[start..start + xml[start..].find('>').expect("a closed tag")];
    let key = format!("{attribute}=\"");
    let at = element
        .find(&format!(" {key}"))
        .or_else(|| element.find(&format!("\n{key}")))
        .map(|i| i + 1)
        .unwrap_or_else(|| panic!("<{tag}> has no {attribute}"));
    let value = &element[at + key.len()..];
    value[..value.find('"').unwrap()].to_string()
}

/// The value of `| field | `value` |` in the listing doc.
fn listed(field: &str) -> String {
    let doc = read("../docs/MICROSOFT-STORE-LISTING.md");
    let row = doc
        .lines()
        .find(|l| l.starts_with(&format!("| {field} |")))
        .unwrap_or_else(|| panic!("no row {field} in MICROSOFT-STORE-LISTING.md"));
    row.split('`').nth(1).expect("a `value`").to_string()
}

/// What scripts/msix-manifest.mjs does with tauri.conf.json's version: X.Y.Z → X.Y.Z.0, each part
/// a 16-bit number, no pre-release.
fn msix_version(version: &str) -> Option<String> {
    let parts: Vec<&str> = version.split('.').collect();
    let plain = parts.len() == 3
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars().all(|c| c.is_ascii_digit())
                && (p.len() == 1 || !p.starts_with('0'))
                && p.parse::<u32>().is_ok_and(|n| n <= 65535)
        });
    plain.then(|| format!("{version}.0"))
}

#[test]
fn the_identity_matches_partner_center_and_the_app() {
    let xml = template();
    assert_eq!(attr(&xml, "Identity", "Name"), listed("Package/Identity/Name"));
    assert_eq!(
        attr(&xml, "Identity", "Publisher"),
        listed("Package/Identity/Publisher")
    );
    assert!(xml.contains(&format!(
        "<PublisherDisplayName>{}</PublisherDisplayName>",
        listed("PublisherDisplayName")
    )));
    assert_eq!(attr(&xml, "Identity", "Name"), msstore::PACKAGE_NAME);
    assert_eq!(attr(&xml, "Identity", "Publisher"), msstore::PUBLISHER);
    assert_eq!(listed("PublisherDisplayName"), msstore::PUBLISHER_DISPLAY_NAME);
    assert_eq!(listed("Package Family Name"), msstore::PACKAGE_FAMILY_NAME);
    assert_eq!(listed("Store ID"), msstore::STORE_ID);
    assert_eq!(attr(&xml, "Identity", "ProcessorArchitecture"), "x64");
    assert_eq!(attr(&xml, "Identity", "Version"), "{{VERSION}}");
}

#[test]
fn the_aumid_startup_task_and_protocol_are_the_ones_the_app_uses() {
    let xml = template();
    assert_eq!(attr(&xml, "Application", "Id"), msstore::APPLICATION_ID);
    assert_eq!(attr(&xml, "Application", "EntryPoint"), "Windows.FullTrustApplication");
    assert_eq!(attr(&xml, "desktop:StartupTask", "TaskId"), msstore::STARTUP_TASK_ID);
    assert_eq!(attr(&xml, "desktop:StartupTask", "Enabled"), "false");
    let config: serde_json::Value = serde_json::from_str(&read("tauri.conf.json")).unwrap();
    assert_eq!(
        config["plugins"]["deep-link"]["desktop"]["schemes"],
        serde_json::json!([attr(&xml, "uap3:Protocol", "Name")])
    );
    assert_eq!(attr(&xml, "uap3:Protocol", "Parameters"), "&quot;%1&quot;");
    assert_eq!(attr(&xml, "TargetDeviceFamily", "MinVersion"), "10.0.17763.0");
    assert_eq!(attr(&xml, "TargetDeviceFamily", "MaxVersionTested"), "10.0.26100.0");
    for capability in [
        r#"<Capability Name="internetClient" />"#,
        r#"<rescap:Capability Name="runFullTrust" />"#,
        r#"<DeviceCapability Name="microphone" />"#,
    ] {
        assert!(xml.contains(capability), "{capability}");
    }
}

#[test]
fn versions_map_to_four_parts() {
    assert_eq!(msix_version("1.2.3").as_deref(), Some("1.2.3.0"));
    assert_eq!(msix_version("0.1.0").as_deref(), Some("0.1.0.0"));
    assert_eq!(msix_version("1.2.65536"), None);
    assert_eq!(msix_version("1.0.0-beta.1"), None);
    assert_eq!(msix_version("1.2"), None);
    // the version a Store package is built with today
    let config: serde_json::Value = serde_json::from_str(&read("tauri.conf.json")).unwrap();
    let version = config["version"].as_str().unwrap();
    assert_eq!(msix_version(version), Some(format!("{version}.0")));
}

#[test]
fn the_store_overlay_drops_the_updater() {
    let overlay: serde_json::Value = serde_json::from_str(&read("tauri.msstore.conf.json")).unwrap();
    // JSON merge patch: null removes plugins.updater (no key, no endpoints in the Store binary)
    assert!(overlay["plugins"]["updater"].is_null());
    assert!(overlay["plugins"].as_object().unwrap().contains_key("updater"));
    // nothing else of the app changes (same identifier: same data folders, keychain entries)
    assert!(overlay.get("identifier").is_none());
    assert!(overlay.get("version").is_none());
}
