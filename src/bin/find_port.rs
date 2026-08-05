use std::error::Error;
use std::io::{self, Write};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use clap::Parser;
#[cfg(target_os = "macos")]
use rerobot::serial_ports::collapse_macos_aliases;
use rerobot::serial_ports::disconnected_ports;
use serialport::{SerialPortInfo, SerialPortType};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Identify the serial port used by a robot controller or motor bus.
#[derive(Debug, Parser)]
#[command(
    version,
    about = "Identify the serial port used by a robot controller or motor bus"
)]
struct Args {
    /// Print the available serial ports without starting the unplug workflow.
    #[arg(long)]
    list: bool,

    /// Time to wait for the operating system to release an unplugged port.
    #[arg(long, default_value_t = 500, value_name = "MILLISECONDS")]
    settle_ms: u64,
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<()> {
    let ports_before = available_ports()?;

    if args.list {
        print_ports(&ports_before);
        return Ok(());
    }

    println!("Finding all available ports for the MotorBus.");
    println!("Ports before disconnecting:");
    print_ports(&ports_before);
    print!("Remove the USB cable from your MotorBus and press Enter when done. ");
    io::stdout().flush()?;

    let mut confirmation = String::new();
    if io::stdin().read_line(&mut confirmation)? == 0 {
        return Err("interactive stdin closed before unplug confirmation".into());
    }
    thread::sleep(Duration::from_millis(args.settle_ms));

    let ports_after = available_ports()?;
    let before_names = port_names(&ports_before);
    let after_names = port_names(&ports_after);
    let removed = disconnected_ports(&before_names, &after_names);

    match removed.as_slice() {
        [port] => {
            println!("The port of this MotorBus is '{port}'");
            println!("Reconnect the USB cable.");
            Ok(())
        }
        [] => Err("could not detect the port: no disconnected port was found".into()),
        ports => Err(format!(
            "could not identify one port: multiple ports disappeared ({})",
            ports.join(", ")
        )
        .into()),
    }
}

fn available_ports() -> Result<Vec<SerialPortInfo>> {
    let mut ports = serialport::available_ports()?;

    #[cfg(target_os = "macos")]
    {
        let preferred_names = collapse_macos_aliases(port_names(&ports));
        ports.retain(|port| preferred_names.binary_search(&port.port_name).is_ok());
    }

    ports.sort_by(|left, right| left.port_name.cmp(&right.port_name));
    ports.dedup_by(|left, right| left.port_name == right.port_name);
    Ok(ports)
}

fn port_names(ports: &[SerialPortInfo]) -> Vec<String> {
    ports.iter().map(|port| port.port_name.clone()).collect()
}

fn print_ports(ports: &[SerialPortInfo]) {
    if ports.is_empty() {
        println!("  (none)");
        return;
    }

    for port in ports {
        println!("  {}{}", port.port_name, port_details(&port.port_type));
    }
}

fn port_details(port_type: &SerialPortType) -> String {
    match port_type {
        SerialPortType::UsbPort(info) => {
            let mut details = vec![format!("USB {:04x}:{:04x}", info.vid, info.pid)];
            if let Some(product) = &info.product {
                details.push(product.clone());
            }
            if let Some(serial) = &info.serial_number {
                details.push(format!("serial {serial}"));
            }
            format!(" ({})", details.join(", "))
        }
        SerialPortType::BluetoothPort => " (Bluetooth)".to_owned(),
        SerialPortType::PciPort => " (PCI)".to_owned(),
        SerialPortType::Unknown => String::new(),
    }
}
