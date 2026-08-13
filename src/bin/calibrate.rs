use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;
use serde_json::{Map, Value};
use spiral_robotics::calibration::{
    Calibration, CalibrationInput, Direction, MotorSpec, RangeTracker, build_calibration,
    validate_motor_specs,
};
use spiral_robotics::calibration_codec;
use spiral_robotics::feetech::FeetechBus;
use spiral_robotics::robot_config::RobotConfig;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const DEFAULT_BAUD_RATE: u32 = 1_000_000;
const SAMPLE_INTERVAL: Duration = Duration::from_millis(20);
const DISPLAY_INTERVAL: Duration = Duration::from_millis(100);

/// Record motor calibration in the robot's single JSON configuration file.
#[derive(Debug, Parser)]
#[command(
    version,
    about = "Calibrate a robot by moving its joints through their full ranges",
    long_about = "Connect to the Feetech motor bus described by the robot JSON file, safely disable torque, and record raw min/max positions while you move every joint by hand. A live terminal table updates until you press Enter. Calibration is saved in the same JSON file as the robot's port, cameras, and motor definitions."
)]
struct Args {
    /// Unified robot configuration to create or update.
    #[arg(long, default_value = "robot.json", value_name = "PATH")]
    config: PathBuf,

    /// Stable name for this physical robot.
    #[arg(long, value_name = "ID")]
    robot_id: Option<String>,

    /// Robot model or hardware profile name.
    #[arg(long, value_name = "TYPE")]
    robot_type: Option<String>,

    /// Serial port to connect to and store in the robot file.
    #[arg(long, value_name = "PORT")]
    port: Option<String>,

    /// Baud rate used by --port and stored in the robot file.
    #[arg(long, requires = "port", value_name = "BITS_PER_SECOND")]
    baud_rate: Option<u32>,

    /// Motor definition NAME=ID[:normal|reversed]. Repeat to add or update motors.
    #[arg(long = "motor", value_name = "SPEC")]
    motors: Vec<MotorArg>,

    /// Import calibration input from JSON instead of connecting to the robot.
    #[arg(long, value_name = "PATH")]
    from_json: Option<String>,

    /// Authorize replacement of existing calibration without prompting.
    #[arg(long)]
    force: bool,

    /// Print the fully merged robot document without writing it.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Clone, Debug)]
struct MotorArg {
    name: String,
    id: u32,
    direction: Option<Direction>,
}

impl FromStr for MotorArg {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let (name, details) = value
            .split_once('=')
            .ok_or_else(|| "expected NAME=ID[:normal|reversed]".to_owned())?;
        let name = name.trim();
        if name.is_empty() {
            return Err("motor name cannot be empty".to_owned());
        }

        let parts = details.split(':').collect::<Vec<_>>();
        if parts.is_empty() || parts.len() > 2 || parts.iter().any(|part| part.is_empty()) {
            return Err("expected NAME=ID[:normal|reversed]".to_owned());
        }

        let id = parts[0]
            .parse::<u32>()
            .map_err(|_| format!("invalid motor ID '{}'", parts[0]))?;
        let direction = parts
            .get(1)
            .map(|value| match *value {
                "normal" => Ok(Direction::Normal),
                "reversed" => Ok(Direction::Reversed),
                _ => Err(format!(
                    "invalid motor direction '{value}'; expected 'normal' or 'reversed'"
                )),
            })
            .transpose()?;

        Ok(Self {
            name: name.to_owned(),
            id,
            direction,
        })
    }
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
    validate_optional_label("--robot-id", args.robot_id.as_deref())?;
    validate_optional_label("--robot-type", args.robot_type.as_deref())?;
    validate_port(args.port.as_deref())?;

    let document = RobotConfig::load(&args.config)?;
    let motors = resolve_motors(&document, args.motors)?;
    validate_motor_specs(&motors)?;
    let port_update = resolve_port(&document, args.port.as_deref(), args.baud_rate)?;

    let has_existing_calibration = has_existing_calibration(&document);
    let batch_input = args.from_json.is_some();
    if batch_replacement_requires_force(
        has_existing_calibration,
        batch_input,
        args.force,
        args.dry_run,
    ) {
        return Err(format!(
            "{} already contains calibration; batch replacement with --from-json requires --force",
            args.config.display()
        )
        .into());
    }
    if has_existing_calibration && !args.force && !args.dry_run {
        if !io::stdin().is_terminal() {
            return Err(format!(
                "{} already contains calibration; pass --force to replace it",
                args.config.display()
            )
            .into());
        }
        if !confirm(
            &mut io::stdin().lock(),
            &mut io::stdout(),
            "This robot file already contains calibration. Replace it? [y/N] ",
        )? {
            return Err("calibration replacement cancelled".into());
        }
    }

    let input = match args.from_json.as_deref() {
        Some(source) => read_calibration_input(source)?,
        None => {
            if !io::stdin().is_terminal() {
                return Err(
                    "interactive calibration requires a terminal; use --from-json PATH for non-interactive input"
                        .into(),
                );
            }
            let serial = live_serial_settings(&document, port_update.as_ref())?;
            if args.dry_run {
                collect_live(&motors, &serial, &mut io::stderr())?
            } else {
                collect_live(&motors, &serial, &mut io::stdout())?
            }
        }
    };
    let calibration = build_calibration(&motors, &input)?;
    let merged = document.merged(
        args.robot_id.as_deref(),
        args.robot_type.as_deref(),
        port_update,
        document.encode_motor_specs(&motors)?,
        calibration_codec::encode_calibration(&calibration)?,
    )?;

    if args.dry_run {
        print!("{}", merged.to_pretty_json()?);
    } else {
        print_summary(&calibration);
        merged.save_atomic_if_unchanged(&args.config, &document)?;
        println!("Calibration saved to {}", args.config.display());
        if args.from_json.is_none() {
            println!("Motor torque remains disabled; support the arm before disconnecting power.");
        }
    }

    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SerialSettings {
    path: String,
    baud_rate: u32,
}

fn live_serial_settings(
    document: &RobotConfig,
    port_update: Option<&Value>,
) -> Result<SerialSettings> {
    let value = port_update.or_else(|| document.root().get("port")).ok_or(
        "live calibration needs a serial port; add port.path to the robot file or pass --port",
    )?;
    let fields = value
        .as_object()
        .ok_or("live calibration needs top-level 'port' to be an object with path and baud_rate")?;
    let path = fields
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty() && !path.contains('\0'))
        .ok_or("live calibration needs port.path to be a non-empty string")?;
    let baud_rate = match fields.get("baud_rate") {
        None => DEFAULT_BAUD_RATE,
        Some(value) => value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or("live calibration needs port.baud_rate to be a positive 32-bit integer")?,
    };
    if let Some(protocol) = fields.get("protocol") {
        let protocol = protocol
            .as_str()
            .ok_or("port.protocol must be a string when present")?;
        if !matches!(protocol, "feetech" | "feetech_protocol_0") {
            return Err(format!(
                "live calibration does not support port protocol {protocol:?}; expected 'feetech'"
            )
            .into());
        }
    }
    Ok(SerialSettings {
        path: path.to_owned(),
        baud_rate,
    })
}

fn validate_optional_label(option: &str, value: Option<&str>) -> Result<()> {
    if let Some(value) = value
        && (value.trim().is_empty() || value.chars().any(char::is_control))
    {
        return Err(format!("{option} must be non-empty and contain no control characters").into());
    }
    Ok(())
}

fn validate_port(port: Option<&str>) -> Result<()> {
    if let Some(port) = port
        && (port.trim().is_empty() || port.contains('\0'))
    {
        return Err("--port must be non-empty and contain no NUL byte".into());
    }
    Ok(())
}

fn resolve_motors(
    document: &RobotConfig,
    arguments: Vec<MotorArg>,
) -> Result<BTreeMap<String, MotorSpec>> {
    let mut motors = match document.motors() {
        None | Some(Value::Null) => BTreeMap::new(),
        Some(_) => document
            .motor_specs()?
            .expect("a present non-null motors value must deserialize to Some"),
    };
    let mut updated_names = BTreeSet::new();
    for argument in arguments {
        if !updated_names.insert(argument.name.clone()) {
            return Err(format!("motor '{}' was specified more than once", argument.name).into());
        }

        if let Some(existing) = motors.get_mut(&argument.name) {
            existing.id = argument.id;
            if let Some(direction) = argument.direction {
                existing.direction = direction;
            }
        } else {
            motors.insert(
                argument.name,
                MotorSpec::new(argument.id, argument.direction.unwrap_or_default()),
            );
        }
    }

    if motors.is_empty() {
        return Err(
            "no motors are configured; add repeated --motor NAME=ID options or a top-level 'motors' object"
                .into(),
        );
    }
    Ok(motors)
}

fn resolve_port(
    document: &RobotConfig,
    path: Option<&str>,
    baud_rate: Option<u32>,
) -> Result<Option<Value>> {
    let Some(path) = path else {
        return Ok(None);
    };

    let mut fields = match document.root().get("port") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(fields)) => fields.clone(),
        Some(_) => {
            return Err(
                "existing top-level 'port' must be a JSON object before it can be updated".into(),
            );
        }
    };
    fields.insert("path".to_owned(), Value::String(path.to_owned()));
    if let Some(baud_rate) = baud_rate {
        fields.insert("baud_rate".to_owned(), Value::from(baud_rate));
    } else {
        fields
            .entry("baud_rate".to_owned())
            .or_insert_with(|| Value::from(DEFAULT_BAUD_RATE));
    }

    Ok(Some(Value::Object(fields)))
}

fn has_existing_calibration(document: &RobotConfig) -> bool {
    document
        .calibration()
        .is_some_and(|calibration| !calibration.is_null())
}

fn batch_replacement_requires_force(
    has_existing_calibration: bool,
    batch_input: bool,
    force: bool,
    dry_run: bool,
) -> bool {
    has_existing_calibration && batch_input && !force && !dry_run
}

fn read_calibration_input(source: &str) -> Result<CalibrationInput> {
    let mut contents = String::new();
    let origin = if source == "-" {
        io::stdin().read_to_string(&mut contents).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("failed to read calibration input from standard input: {error}"),
            )
        })?;
        "standard input".to_owned()
    } else {
        contents = fs::read_to_string(source).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("failed to read calibration input from '{source}': {error}"),
            )
        })?;
        format!("'{source}'")
    };
    parse_calibration_input(&contents, &origin)
}

fn parse_calibration_input(contents: &str, origin: &str) -> Result<CalibrationInput> {
    if contents.trim().is_empty() {
        return Err(format!("calibration input from {origin} is empty").into());
    }
    calibration_codec::decode_input(contents).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid calibration JSON from {origin}: {error}"),
        )
        .into()
    })
}

fn collect_live<W>(
    motors: &BTreeMap<String, MotorSpec>,
    serial: &SerialSettings,
    output: &mut W,
) -> Result<CalibrationInput>
where
    W: Write,
{
    writeln!(
        output,
        "Connecting to {} at {} baud...",
        serial.path, serial.baud_rate
    )?;
    output.flush()?;
    let mut bus = FeetechBus::connect(&serial.path, serial.baud_rate, motors)?;
    let positions = bus.read_positions().map_err(|error| {
        format!(
            "could not verify all configured motors on {}: {error}",
            serial.path
        )
    })?;
    writeln!(
        output,
        "Connected to {} motors. Disabling torque so the arm can be moved safely...",
        positions.len()
    )?;
    output.flush()?;
    bus.disable_torque()?;

    // Start the range after torque has actually been disabled. Reading once
    // before the write above is only a connection/identity check.
    let mut tracker = RangeTracker::new(bus.read_positions()?)?;
    writeln!(output, "Torque disabled.")?;
    writeln!(
        output,
        "Move every joint through its full range. Min/max values are recorded automatically."
    )?;
    writeln!(
        output,
        "Press Enter when all joints have reached both limits.\n"
    )?;

    let mut ordered_motors = motors.iter().collect::<Vec<_>>();
    ordered_motors.sort_by(|(left_name, left), (right_name, right)| {
        left.id
            .cmp(&right.id)
            .then_with(|| left_name.cmp(right_name))
    });

    let (done_sender, done_receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        let result = io::stdin().read_line(&mut line);
        let _ = done_sender.send(result);
    });

    render_live_table(output, &ordered_motors, &tracker, false)?;
    let mut last_display = Instant::now();
    loop {
        match done_receiver.try_recv() {
            Ok(Ok(0)) => return Err("interactive input closed before calibration finished".into()),
            Ok(Ok(_)) => break,
            Ok(Err(error)) => return Err(error.into()),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                return Err("calibration input listener stopped unexpectedly".into());
            }
        }

        let started = Instant::now();
        tracker.observe(&bus.read_positions()?)?;
        if last_display.elapsed() >= DISPLAY_INTERVAL {
            render_live_table(output, &ordered_motors, &tracker, true)?;
            last_display = Instant::now();
        }
        if let Some(remaining) = SAMPLE_INTERVAL.checked_sub(started.elapsed()) {
            thread::sleep(remaining);
        }
    }

    render_live_table(output, &ordered_motors, &tracker, true)?;
    writeln!(output)?;
    Ok(tracker.into_input()?)
}

fn render_live_table<W: Write>(
    output: &mut W,
    motors: &[(&String, &MotorSpec)],
    tracker: &RangeTracker,
    redraw: bool,
) -> io::Result<()> {
    if redraw {
        write!(output, "\x1b[{}A", motors.len() + 2)?;
    }
    writeln!(
        output,
        "\r\x1b[2K{:<20} {:>4} {:>9} {:>9} {:>9} {:>9}",
        "NAME", "ID", "MIN", "POS", "MAX", "SPAN"
    )?;
    writeln!(output, "\r\x1b[2K{}", "-".repeat(64))?;
    for (name, motor) in motors {
        let range = tracker
            .ranges()
            .get(*name)
            .expect("every configured motor must have a live range");
        writeln!(
            output,
            "\r\x1b[2K{name:<20} {:>4} {:>9} {:>9} {:>9} {:>9}",
            motor.id,
            range.minimum,
            range.position,
            range.maximum,
            range.span()
        )?;
    }
    output.flush()
}

fn confirm<R, W>(input: &mut R, output: &mut W, prompt: &str) -> Result<bool>
where
    R: BufRead,
    W: Write,
{
    write!(output, "{prompt}")?;
    output.flush()?;

    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Err("interactive input closed before confirmation".into());
    }
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn print_summary(calibration: &Calibration) {
    println!("\n--- Motor Calibration ---");
    println!(
        "{:<20} {:>5} {:>10} {:>10} {:>10} {:>10}",
        "NAME", "ID", "DIRECTION", "CENTER", "MIN", "MAX"
    );
    let mut ordered_motors = calibration.motors.iter().collect::<Vec<_>>();
    ordered_motors.sort_by(|(left_name, left), (right_name, right)| {
        left.id
            .cmp(&right.id)
            .then_with(|| left_name.cmp(right_name))
    });
    for (name, motor) in ordered_motors {
        println!(
            "{name:<20} {:>5} {:>10} {:>10} {:>10} {:>10}",
            motor.id,
            direction_label(motor.direction),
            motor.center,
            motor.range_min,
            motor.range_max
        );
    }
    println!("-------------------------");
}

fn direction_label(direction: Direction) -> &'static str {
    match direction {
        Direction::Normal => "normal",
        Direction::Reversed => "reversed",
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use serde_json::json;

    use super::*;

    #[test]
    fn parses_motor_spec_without_overriding_direction() {
        let parsed: MotorArg = "shoulder_pan=1".parse().expect("motor should parse");
        assert_eq!(parsed.name, "shoulder_pan");
        assert_eq!(parsed.id, 1);
        assert_eq!(parsed.direction, None);
    }

    #[test]
    fn parses_explicit_motor_direction() {
        let parsed: MotorArg = "wrist_roll=5:reversed".parse().expect("motor should parse");
        assert_eq!(parsed.id, 5);
        assert_eq!(parsed.direction, Some(Direction::Reversed));

        let parsed: MotorArg = "wrist_roll=5:normal".parse().expect("motor should parse");
        assert_eq!(parsed.direction, Some(Direction::Normal));
    }

    #[test]
    fn rejects_malformed_motor_specs() {
        for value in [
            "joint",
            "=1",
            "joint=x",
            "joint=1:",
            "joint=1:reverse",
            "joint=1:4096:0",
        ] {
            assert!(value.parse::<MotorArg>().is_err(), "accepted {value}");
        }
    }

    #[test]
    fn motor_updates_merge_and_preserve_existing_metadata() {
        let document = RobotConfig::from_value(json!({
            "motors": {
                "shoulder": {
                    "id": 1,
                    "direction": "reversed",
                    "model": "sts3215"
                },
                "elbow": {
                    "id": 2,
                    "direction": "normal",
                    "model": "sts3215"
                }
            }
        }))
        .expect("valid robot document");

        let merged = resolve_motors(
            &document,
            vec!["shoulder=7".parse().unwrap(), "wrist=3".parse().unwrap()],
        )
        .expect("merge motors");

        assert_eq!(merged.len(), 3);
        assert_eq!(merged["shoulder"].id, 7);
        assert_eq!(merged["shoulder"].direction, Direction::Reversed);
        assert_eq!(merged["shoulder"].model.as_deref(), Some("sts3215"));
        assert_eq!(merged["elbow"].id, 2);
        assert_eq!(merged["elbow"].model.as_deref(), Some("sts3215"));
        assert_eq!(merged["wrist"].direction, Direction::Normal);
    }

    #[test]
    fn explicit_direction_changes_existing_motor() {
        let document = RobotConfig::from_value(json!({
            "motors": {
                "joint": {"id": 1, "direction": "reversed", "model": "custom"}
            }
        }))
        .expect("valid robot document");

        let merged = resolve_motors(&document, vec!["joint=4:normal".parse().unwrap()])
            .expect("merge motor");

        assert_eq!(merged["joint"].id, 4);
        assert_eq!(merged["joint"].direction, Direction::Normal);
        assert_eq!(merged["joint"].model.as_deref(), Some("custom"));
    }

    #[test]
    fn rejects_duplicate_motor_updates() {
        let error = resolve_motors(
            &RobotConfig::default(),
            vec!["joint=1".parse().unwrap(), "joint=2".parse().unwrap()],
        )
        .expect_err("duplicate update must fail");

        assert!(error.to_string().contains("more than once"));
    }

    #[test]
    fn live_table_shows_min_position_max_and_span() {
        let motors = BTreeMap::from([(
            "joint".to_owned(),
            MotorSpec {
                id: 7,
                direction: Direction::Normal,
                model: None,
            },
        )]);
        let ordered = motors.iter().collect::<Vec<_>>();
        let tracker = RangeTracker::new(BTreeMap::from([("joint".to_owned(), 123)])).unwrap();
        let mut output = Vec::new();

        render_live_table(&mut output, &ordered, &tracker, false).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("MIN"));
        assert!(output.contains("POS"));
        assert!(output.contains("MAX"));
        assert!(output.contains("SPAN"));
        assert!(output.contains("joint"));
        assert!(output.contains("123"));
    }

    #[test]
    fn eof_cancels_confirmation() {
        let error = confirm(
            &mut Cursor::new(Vec::<u8>::new()),
            &mut Vec::new(),
            "Continue? ",
        )
        .expect_err("EOF should cancel");
        assert!(error.to_string().contains("closed"));
    }

    #[test]
    fn port_update_preserves_existing_fields_and_baud_rate() {
        let document = RobotConfig::from_value(json!({
            "port": {
                "path": "/dev/old",
                "baud_rate": 57600,
                "transport": "usb"
            }
        }))
        .expect("valid robot document");

        let value = resolve_port(&document, Some("/dev/new"), None)
            .expect("merge port")
            .expect("updated port");

        assert_eq!(value["path"], "/dev/new");
        assert_eq!(value["baud_rate"], 57_600);
        assert_eq!(value["transport"], "usb");
    }

    #[test]
    fn new_port_gets_default_or_explicit_baud_rate() {
        let default = resolve_port(&RobotConfig::default(), Some("/dev/ttyACM0"), None)
            .expect("build port")
            .expect("new port");
        assert_eq!(default["baud_rate"], DEFAULT_BAUD_RATE);

        let explicit = resolve_port(&RobotConfig::default(), Some("/dev/ttyACM0"), Some(115_200))
            .expect("build port")
            .expect("new port");
        assert_eq!(explicit["baud_rate"], 115_200);
    }

    #[test]
    fn live_serial_settings_use_config_or_the_cli_override() {
        let document = RobotConfig::from_value(json!({
            "port": {"path": "/dev/original", "baud_rate": 57600}
        }))
        .unwrap();
        assert_eq!(
            live_serial_settings(&document, None).unwrap(),
            SerialSettings {
                path: "/dev/original".to_owned(),
                baud_rate: 57_600
            }
        );

        let update = resolve_port(&document, Some("/dev/new"), Some(1_000_000))
            .unwrap()
            .unwrap();
        assert_eq!(
            live_serial_settings(&document, Some(&update)).unwrap(),
            SerialSettings {
                path: "/dev/new".to_owned(),
                baud_rate: 1_000_000
            }
        );
    }

    #[test]
    fn live_serial_settings_reject_missing_or_unsupported_ports() {
        let missing = live_serial_settings(&RobotConfig::default(), None)
            .expect_err("missing port must fail");
        assert!(missing.to_string().contains("--port"));

        let unsupported = RobotConfig::from_value(json!({
            "port": {"path": "/dev/bus", "protocol": "dynamixel"}
        }))
        .unwrap();
        assert!(
            live_serial_settings(&unsupported, None)
                .expect_err("unsupported protocol must fail")
                .to_string()
                .contains("does not support")
        );
    }

    #[test]
    fn null_calibration_is_absent() {
        let null = RobotConfig::from_value(json!({"calibration": null})).unwrap();
        let populated = RobotConfig::from_value(json!({"calibration": {"motors": {}}})).unwrap();

        assert!(!has_existing_calibration(&null));
        assert!(has_existing_calibration(&populated));
    }

    #[test]
    fn batch_replacement_requires_force_but_dry_run_does_not() {
        assert!(batch_replacement_requires_force(true, true, false, false));
        assert!(!batch_replacement_requires_force(true, true, true, false));
        assert!(!batch_replacement_requires_force(true, true, false, true));
        assert!(!batch_replacement_requires_force(true, false, false, false));
        assert!(!batch_replacement_requires_force(false, true, false, false));
    }

    #[test]
    fn calibration_input_errors_include_the_source() {
        let parse_error =
            parse_calibration_input("{", "'readings.json'").expect_err("invalid JSON must fail");
        assert!(parse_error.to_string().contains("readings.json"));
        assert!(parse_error.to_string().contains("invalid calibration JSON"));

        let directory = tempfile::tempdir().expect("temporary directory");
        let missing = directory.path().join("missing-input.json");
        let read_error =
            read_calibration_input(missing.to_str().unwrap()).expect_err("missing input must fail");
        assert!(read_error.to_string().contains("missing-input.json"));
        assert!(read_error.to_string().contains("failed to read"));
    }
}
