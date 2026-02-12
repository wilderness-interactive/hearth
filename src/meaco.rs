use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

// -- Meaco Arete Two 25L — Actual DPS mapping --
//
// Confirmed via TinyTuya wizard + local device poll (2026-02-12).
//
// | DPS | Code                  | Type    | Notes                                      |
// |-----|-----------------------|---------|--------------------------------------------|
// |  1  | switch                | Boolean | Power on/off                               |
// |  2  | dehumidify_set_value  | Integer | Target humidity 35-70%, step 5             |
// |  4  | (unlisted)            | String  | Operating mode. Seen: "manual"             |
// | 14  | child_lock            | Boolean |                                            |
// | 16  | humidity_indoor       | Integer | Current humidity 0-100%                    |
// | 17  | countdown_set         | Enum    | "cancel", "1h", "2h", "3h"                |
// | 18  | countdown_left        | Integer | Hours remaining on countdown, 0-24         |
// | 19  | fault                 | Bitmap  | tankfull, defrost, E1, E2, L2, L3, L4, wet |
// |101  | (unlisted)            | String  | Unknown. Seen: "cancel"                    |
//
// Device ID:  REDACTED_DEVICE_ID
// Model:      MeacoDryArete2-25L
// Protocol:   v3.3
// IP:         REDACTED_IP (DHCP — may change)
// MAC:        REDACTED_MAC

/// Operating mode.
///
/// DPS 4 — only "manual" confirmed from device poll. Other values
/// are reasonable guesses for the Meaco Arete 2 and may need updating
/// once tested against the real device.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Manual,
    Auto,
    Drying,
    Continuous,
}

/// Countdown timer setting.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub enum Countdown {
    #[serde(rename = "cancel")]
    Cancel,
    #[serde(rename = "1h")]
    OneHour,
    #[serde(rename = "2h")]
    TwoHours,
    #[serde(rename = "3h")]
    ThreeHours,
}

/// Fault bitmap flags (DPS 19).
/// Bit 0 = tankfull, bit 1 = defrost, bit 2 = E1, bit 3 = E2,
/// bit 4 = L2, bit 5 = L3, bit 6 = L4, bit 7 = wet.
const FAULT_LABELS: &[&str] = &["tankfull", "defrost", "E1", "E2", "L2", "L3", "L4", "wet"];

/// Current dehumidifier status — a read-only snapshot of device data.
#[derive(Debug, Clone, Serialize)]
pub struct DehumidifierStatus {
    pub power: bool,
    pub target_humidity: u32,
    pub mode: Option<Mode>,
    pub current_humidity: Option<u32>,
    pub child_lock: Option<bool>,
    pub countdown: Option<Countdown>,
    pub countdown_left: Option<u32>,
    pub fault: Option<u32>,
}

#[derive(Debug)]
pub enum DpsError {
    MissingField(&'static str),
    InvalidValue { field: &'static str, raw: String },
    HumidityOutOfRange(u32),
}

impl fmt::Display for DpsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DpsError::MissingField(name) => write!(f, "Missing required DPS field: {name}"),
            DpsError::InvalidValue { field, raw } => {
                write!(f, "Invalid value for DPS {field}: {raw}")
            }
            DpsError::HumidityOutOfRange(v) => {
                write!(f, "Humidity {v} out of range (35-70, step 5)")
            }
        }
    }
}

impl std::error::Error for DpsError {}

// -- Parsing device DPS JSON into typed status --

/// Parse a DPS JSON object from the device into typed status.
/// DPS keys are string numbers: "1", "2", "4", etc.
/// Fields that aren't present in the response are set to None.
pub fn parse_status(dps: &serde_json::Value) -> Result<DehumidifierStatus, DpsError> {
    let power = dps
        .get("1")
        .and_then(|v| v.as_bool())
        .ok_or(DpsError::MissingField("1 (switch)"))?;

    let target_humidity = dps
        .get("2")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .ok_or(DpsError::MissingField("2 (dehumidify_set_value)"))?;

    let mode = dps.get("4").and_then(|v| v.as_str()).map(parse_mode).transpose()?;
    let current_humidity = dps.get("16").and_then(|v| v.as_u64()).map(|v| v as u32);
    let child_lock = dps.get("14").and_then(|v| v.as_bool());
    let countdown = dps.get("17").and_then(|v| v.as_str()).map(parse_countdown).transpose()?;
    let countdown_left = dps.get("18").and_then(|v| v.as_u64()).map(|v| v as u32);
    let fault = dps.get("19").and_then(|v| v.as_u64()).map(|v| v as u32);

    Ok(DehumidifierStatus {
        power,
        target_humidity,
        mode,
        current_humidity,
        child_lock,
        countdown,
        countdown_left,
        fault,
    })
}

fn parse_mode(s: &str) -> Result<Mode, DpsError> {
    match s {
        "manual" => Ok(Mode::Manual),
        "auto" => Ok(Mode::Auto),
        "drying" => Ok(Mode::Drying),
        "continuous" => Ok(Mode::Continuous),
        other => Err(DpsError::InvalidValue {
            field: "4 (mode)",
            raw: other.to_owned(),
        }),
    }
}

fn parse_countdown(s: &str) -> Result<Countdown, DpsError> {
    match s {
        "cancel" => Ok(Countdown::Cancel),
        "1h" => Ok(Countdown::OneHour),
        "2h" => Ok(Countdown::TwoHours),
        "3h" => Ok(Countdown::ThreeHours),
        other => Err(DpsError::InvalidValue {
            field: "17 (countdown_set)",
            raw: other.to_owned(),
        }),
    }
}

// -- Building DPS JSON for sending to the device --

pub fn build_power_dps(on: bool) -> serde_json::Value {
    serde_json::json!({"1": on})
}

pub fn build_target_humidity_dps(value: u32) -> Result<serde_json::Value, DpsError> {
    if value < 35 || value > 70 || value % 5 != 0 {
        return Err(DpsError::HumidityOutOfRange(value));
    }
    Ok(serde_json::json!({"2": value}))
}

pub fn build_mode_dps(mode: &Mode) -> serde_json::Value {
    let val = match mode {
        Mode::Manual => "manual",
        Mode::Auto => "auto",
        Mode::Drying => "drying",
        Mode::Continuous => "continuous",
    };
    serde_json::json!({"4": val})
}

pub fn build_child_lock_dps(locked: bool) -> serde_json::Value {
    serde_json::json!({"14": locked})
}

pub fn build_countdown_dps(countdown: &Countdown) -> serde_json::Value {
    let val = match countdown {
        Countdown::Cancel => "cancel",
        Countdown::OneHour => "1h",
        Countdown::TwoHours => "2h",
        Countdown::ThreeHours => "3h",
    };
    serde_json::json!({"17": val})
}

/// Decode the fault bitmap into a list of active fault names.
fn decode_faults(bitmap: u32) -> Vec<&'static str> {
    FAULT_LABELS
        .iter()
        .enumerate()
        .filter(|(i, _)| bitmap & (1 << i) != 0)
        .map(|(_, label)| *label)
        .collect()
}

/// Format a DehumidifierStatus as a human-readable summary.
pub fn format_status(status: &DehumidifierStatus) -> String {
    let mut lines = Vec::new();

    lines.push(format!(
        "Power: {}",
        if status.power { "ON" } else { "OFF" }
    ));

    if let Some(h) = status.current_humidity {
        lines.push(format!("Current humidity: {h}%"));
    }
    lines.push(format!("Target humidity: {}%", status.target_humidity));

    if let Some(ref mode) = status.mode {
        lines.push(format!("Mode: {mode:?}"));
    }

    if let Some(ref countdown) = status.countdown {
        lines.push(format!("Timer: {countdown:?}"));
    }

    if let Some(left) = status.countdown_left {
        if left > 0 {
            lines.push(format!("Time remaining: {left}h"));
        }
    }

    if let Some(locked) = status.child_lock {
        lines.push(format!(
            "Child lock: {}",
            if locked { "ON" } else { "OFF" }
        ));
    }

    if let Some(fault) = status.fault {
        if fault != 0 {
            let names = decode_faults(fault);
            lines.push(format!("FAULTS: {}", names.join(", ")));
        }
    }

    lines.join("\n")
}
