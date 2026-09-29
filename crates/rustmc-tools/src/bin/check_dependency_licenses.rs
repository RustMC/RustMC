use rustmc_tools::{REVIEWED, review_packages};
use serde_json::Value;
use std::{error::Error, process::Command};

fn main() -> Result<(), Box<dyn Error>> {
    let result = Command::new("cargo")
        .args(["metadata", "--locked", "--format-version", "1"])
        .output()?;
    if !result.status.success() {
        return Err(String::from_utf8_lossy(&result.stderr).into_owned().into());
    }
    let metadata: Value = serde_json::from_slice(&result.stdout)?;
    let packages = metadata["packages"]
        .as_array()
        .ok_or("missing packages in Cargo metadata")?;
    let problems = review_packages(packages, REVIEWED);
    if !problems.is_empty() {
        return Err(format!(
            "dependency license review required:\n- {}",
            problems.join("\n- ")
        )
        .into());
    }
    println!(
        "Reviewed declared licenses for {} locked third-party packages.",
        packages.len() - 2
    );
    println!("This does not replace notice/source review before binary distribution.");
    Ok(())
}
