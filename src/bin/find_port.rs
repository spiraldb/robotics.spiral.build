use std::error::Error;
use std::io::{self, Write};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use clap::Parser;
use serialport::SerialPortInfo;
use spiral_robotics::serial_ports::{
    available_ports, disconnected_ports, port_description, port_names,
};

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

fn print_ports(ports: &[SerialPortInfo]) {
    if ports.is_empty() {
        println!("  (none)");
        return;
    }

    for port in ports {
        match port_description(&port.port_type) {
            Some(details) => println!("  {} ({details})", port.port_name),
            None => println!("  {}", port.port_name),
        }
    }
}
