use std::process::Command;
use serde_json::Value;

fn run_electrum_cmd(args: &[&str]) -> Result<Value, Box<dyn std::error::Error>> {
    let python_path = "/usr/bin/python3";
    let electrum_script = "/home/deus/Electrum-4.5.2/run_electrum";

    let output = Command::new(python_path)
        .arg(electrum_script)
        .args(args)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Electrum command failed: {}", stderr).into());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    
    // Parse stdout JSON output directly into a serde_json Value
    let json_res: Value = serde_json::from_str(&stdout)?;
    Ok(json_res)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Fetching addresses via run_electrum CLI...");

    // Calls: python3 /home/deus/Electrum-4.5.2/run_electrum listaddresses
    let addresses = run_electrum_cmd(&["listaddresses"])?;
    println!("Addresses:\n{:#?}", addresses);

    // Calls: python3 /home/deus/Electrum-4.5.2/run_electrum getunusedaddress
    let next_address = run_electrum_cmd(&["getunusedaddress"])?;
    println!("\nNext Unused Address: {}", next_address);

    Ok(())
}
