//! Rebindable keyboard controls. The arrow keys (and Ctrl for duck) stay as fixed secondary keys; the primary keys below
//! can be changed on the Controls screen and are stored in the lobby prefs file as key names.

use bevy::prelude::KeyCode;
use std::sync::Mutex;

/// Order matches [`ACTIONS`].
pub const DEFAULTS: [KeyCode; 8] = [
    KeyCode::KeyW,
    KeyCode::KeyS,
    KeyCode::KeyA,
    KeyCode::KeyD,
    KeyCode::Space,
    KeyCode::KeyC,
    KeyCode::KeyF,
    KeyCode::ShiftLeft,
];

pub const ACTIONS: [&str; 8] = ["Up", "Down", "Left", "Right", "Jump", "Duck", "Kick", "Lift"];

static KEYS: Mutex<[KeyCode; 8]> = Mutex::new(DEFAULTS);

pub fn get() -> [KeyCode; 8] {
    *KEYS.lock().unwrap()
}

pub fn set(action: usize, key: KeyCode) {
    if let Some(slot) = KEYS.lock().unwrap().get_mut(action) {
        *slot = key;
    }
}

pub fn reset() {
    *KEYS.lock().unwrap() = DEFAULTS;
}

/// Every key a player can pick, with its display name.
const TABLE: &[(KeyCode, &str)] = &[
    (KeyCode::KeyA, "A"), (KeyCode::KeyB, "B"), (KeyCode::KeyC, "C"), (KeyCode::KeyD, "D"), (KeyCode::KeyE, "E"),
    (KeyCode::KeyF, "F"), (KeyCode::KeyG, "G"), (KeyCode::KeyH, "H"), (KeyCode::KeyI, "I"), (KeyCode::KeyJ, "J"),
    (KeyCode::KeyK, "K"), (KeyCode::KeyL, "L"), (KeyCode::KeyM, "M"), (KeyCode::KeyN, "N"), (KeyCode::KeyO, "O"),
    (KeyCode::KeyP, "P"), (KeyCode::KeyQ, "Q"), (KeyCode::KeyR, "R"), (KeyCode::KeyS, "S"), (KeyCode::KeyT, "T"),
    (KeyCode::KeyU, "U"), (KeyCode::KeyV, "V"), (KeyCode::KeyW, "W"), (KeyCode::KeyX, "X"), (KeyCode::KeyY, "Y"),
    (KeyCode::KeyZ, "Z"),
    (KeyCode::Digit0, "0"), (KeyCode::Digit1, "1"), (KeyCode::Digit2, "2"), (KeyCode::Digit3, "3"),
    (KeyCode::Digit4, "4"), (KeyCode::Digit5, "5"), (KeyCode::Digit6, "6"), (KeyCode::Digit7, "7"),
    (KeyCode::Digit8, "8"), (KeyCode::Digit9, "9"),
    (KeyCode::Space, "Space"), (KeyCode::ShiftLeft, "Left Shift"), (KeyCode::ShiftRight, "Right Shift"),
    (KeyCode::ControlLeft, "Left Ctrl"), (KeyCode::ControlRight, "Right Ctrl"), (KeyCode::AltLeft, "Left Alt"),
    (KeyCode::Tab, "Tab"), (KeyCode::Enter, "Enter"), (KeyCode::Backspace, "Backspace"),
    (KeyCode::ArrowUp, "Up Arrow"), (KeyCode::ArrowDown, "Down Arrow"), (KeyCode::ArrowLeft, "Left Arrow"),
    (KeyCode::ArrowRight, "Right Arrow"),
    (KeyCode::Comma, ","), (KeyCode::Period, "."), (KeyCode::Slash, "/"), (KeyCode::Semicolon, ";"),
    (KeyCode::Quote, "'"), (KeyCode::BracketLeft, "["), (KeyCode::BracketRight, "]"), (KeyCode::Minus, "-"),
    (KeyCode::Equal, "="), (KeyCode::Backquote, "`"),
    (KeyCode::Numpad0, "Num 0"), (KeyCode::Numpad1, "Num 1"), (KeyCode::Numpad2, "Num 2"), (KeyCode::Numpad3, "Num 3"),
    (KeyCode::Numpad4, "Num 4"), (KeyCode::Numpad5, "Num 5"), (KeyCode::Numpad6, "Num 6"), (KeyCode::Numpad7, "Num 7"),
    (KeyCode::Numpad8, "Num 8"), (KeyCode::Numpad9, "Num 9"),
];

pub fn name(key: KeyCode) -> &'static str {
    TABLE.iter().find(|(k, _)| *k == key).map_or("?", |(_, n)| n)
}

/// True for keys that can be bound (Escape cancels the capture and is never bound).
pub fn bindable(key: KeyCode) -> bool {
    TABLE.iter().any(|(k, _)| *k == key)
}

/// Restores the bindings from the prefs file's `keys` object (action name -> key name).
pub fn load(prefs: &serde_json::Value) {
    for (i, action) in ACTIONS.iter().enumerate() {
        if let Some(n) = prefs["keys"][*action].as_str() {
            if let Some((k, _)) = TABLE.iter().find(|(_, name)| *name == n) {
                set(i, *k);
            }
        }
    }
}

pub fn to_prefs() -> serde_json::Value {
    let keys = get();
    let mut map = serde_json::Map::new();
    for (i, action) in ACTIONS.iter().enumerate() {
        map.insert((*action).into(), name(keys[i]).into());
    }
    serde_json::Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_names_and_round_trip() {
        for k in DEFAULTS {
            assert!(bindable(k), "{k:?}");
        }
        reset();
        let saved = serde_json::json!({ "keys": to_prefs() });
        set(0, KeyCode::KeyI);
        load(&saved);
        assert_eq!(get()[0], KeyCode::KeyW);
    }
}
