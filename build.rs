use std::path::Path;
use std::process::Command;

fn main() {
    // Only rebuild the Go helper when its source changes.
    println!("cargo:rerun-if-changed=go-helper/main.go");
    println!("cargo:rerun-if-changed=go-helper/go.mod");
    println!("cargo:rerun-if-changed=go-helper/go.sum");

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let helper_bin = Path::new(&out_dir).join("cadence-trace-helper");

    // Try to build the Go helper.  If `go` is not available (e.g. in a
    // minimal CI environment) we skip the build — the integration tests
    // will still fail at runtime with a clear message about the missing
    // binary.
    let status = Command::new("go")
        .args(["build", "-o", helper_bin.to_str().unwrap(), "."])
        .current_dir("go-helper")
        .status();

    match status {
        Ok(s) if s.success() => {
            // Tell downstream code where to find the binary.
            println!(
                "cargo:rustc-env=CADENCE_HELPER_BIN_BUILT={}",
                helper_bin.display()
            );
        }
        Ok(s) => {
            println!(
                "cargo:warning=Go helper build exited with status {}; \
                 integration tests will need CADENCE_HELPER_BIN set manually",
                s
            );
        }
        Err(e) => {
            println!(
                "cargo:warning=Could not run `go build`: {}; \
                 integration tests will need CADENCE_HELPER_BIN set manually",
                e
            );
        }
    }
}
