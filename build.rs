use std::{borrow::Cow, fs};

const LAUNCH_PLIST: &str = include_str!("./resources/org.blackholefox.keeperofkeys.plist");
const PATH_TEMPLATE: &str = "$BINARY_PATH";

fn main() {
    let binary_parent_path = if cfg!(debug_assertions) {
        let project_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        Cow::Owned(format!("{project_dir}/target/debug/bundle/osx"))
    } else {
        Cow::Borrowed("/Applications")
    };

    let binary_path =
        format!("{binary_parent_path}/Keeper of Keys.app/Contents/MacOS/keeper_of_keys");

    let contents = LAUNCH_PLIST.replace(PATH_TEMPLATE, &binary_path);

    // this should really use `OUT_DIR` but `embed_plist` can't use non-literals
    // so the path needs to be deterministic.
    fs::write("./target/launchd.plist", contents).unwrap()
}
