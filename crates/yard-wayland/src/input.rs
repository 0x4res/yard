//! Input handling for YARD Wayland integration.
//!
//! This module provides scancode translation from Linux evdev keycodes
//! (as provided by Wayland) to RDP scancodes.

/// Translates a Wayland raw keycode (evdev) to an RDP scancode.
///
/// Wayland provides raw keycodes from the Linux evdev subsystem.
/// RDP uses its own scancode format based on PS/2 scancodes:
/// - Standard keys: 0x00-0x7F (single-byte scancodes)
/// - Extended keys: 0xE0xx where 0xE0 is the prefix and xx is the scancode
///
/// # Arguments
///
/// * `raw_keycode` - The raw keycode from Wayland's KeyEvent (evdev keycode)
///
/// # Returns
///
/// * `Some(scancode)` - The corresponding RDP scancode
/// * `None` - If the keycode cannot be translated (unknown or unsupported)
///
/// # Note
///
/// Wayland's raw keycodes are evdev keycodes. X11 keycodes = evdev + 8.
/// evdev keycodes map closely to PS/2 scancodes for standard keys.
pub fn wayland_to_rdp_scancode(raw_keycode: u32) -> Option<u16> {
    // Wayland raw keycodes are evdev keycodes
    // For most standard keys, evdev keycode == PS/2 scancode
    // Extended keys need the 0xE0 prefix

    // Handle extended keys first (those that need 0xE0 prefix)
    let extended = match raw_keycode {
        // Arrow keys
        103 => Some(0x48), // KEY_UP
        105 => Some(0x4B), // KEY_LEFT
        106 => Some(0x4D), // KEY_RIGHT
        108 => Some(0x50), // KEY_DOWN

        // Navigation cluster
        102 => Some(0x47), // KEY_HOME
        107 => Some(0x4F), // KEY_END
        104 => Some(0x49), // KEY_PAGEUP
        109 => Some(0x51), // KEY_PAGEDOWN
        110 => Some(0x52), // KEY_INSERT
        111 => Some(0x53), // KEY_DELETE

        // Keypad keys that differ when NumLock is off
        // (These are standard when NumLock is on, extended when off)

        // Right-side modifier keys
        97 => Some(0x1D),  // KEY_RIGHTCTRL
        100 => Some(0x38), // KEY_RIGHTALT

        // Other extended keys
        125 => Some(0x5B), // KEY_LEFTMETA (Windows key)
        126 => Some(0x5C), // KEY_RIGHTMETA
        127 => Some(0x5D), // KEY_COMPOSE (Menu/Apps key)

        // Print Screen / SysRq
        99 => Some(0x37), // KEY_SYSRQ (Print Screen)

        // Pause/Break is special (0xE1 prefix), but we map it simply
        119 => Some(0x46), // KEY_PAUSE (simplified mapping)

        // Keypad Enter and Keypad /
        96 => Some(0x1C), // KEY_KPENTER
        98 => Some(0x35), // KEY_KPSLASH

        _ => None,
    };

    if let Some(sc) = extended {
        // Extended keys get 0xE0 prefix in high byte
        return Some(0xE000 | sc);
    }

    // Standard keys: direct mapping from evdev to PS/2 scancode
    // Most standard keys have identical codes
    match raw_keycode {
        // Escape and function keys
        1 => Some(0x01),   // KEY_ESC
        59 => Some(0x3B),  // KEY_F1
        60 => Some(0x3C),  // KEY_F2
        61 => Some(0x3D),  // KEY_F3
        62 => Some(0x3E),  // KEY_F4
        63 => Some(0x3F),  // KEY_F5
        64 => Some(0x40),  // KEY_F6
        65 => Some(0x41),  // KEY_F7
        66 => Some(0x42),  // KEY_F8
        67 => Some(0x43),  // KEY_F9
        68 => Some(0x44),  // KEY_F10
        87 => Some(0x57),  // KEY_F11
        88 => Some(0x58),  // KEY_F12
        183 => Some(0x64), // KEY_F13
        184 => Some(0x65), // KEY_F14
        185 => Some(0x66), // KEY_F15
        186 => Some(0x67), // KEY_F16
        187 => Some(0x68), // KEY_F17
        188 => Some(0x69), // KEY_F18
        189 => Some(0x6A), // KEY_F19
        190 => Some(0x6B), // KEY_F20
        191 => Some(0x6C), // KEY_F21
        192 => Some(0x6D), // KEY_F22
        193 => Some(0x6E), // KEY_F23
        194 => Some(0x6F), // KEY_F24

        // Number row
        2 => Some(0x02),  // KEY_1
        3 => Some(0x03),  // KEY_2
        4 => Some(0x04),  // KEY_3
        5 => Some(0x05),  // KEY_4
        6 => Some(0x06),  // KEY_5
        7 => Some(0x07),  // KEY_6
        8 => Some(0x08),  // KEY_7
        9 => Some(0x09),  // KEY_8
        10 => Some(0x0A), // KEY_9
        11 => Some(0x0B), // KEY_0
        12 => Some(0x0C), // KEY_MINUS
        13 => Some(0x0D), // KEY_EQUAL
        14 => Some(0x0E), // KEY_BACKSPACE

        // Tab and top letter row
        15 => Some(0x0F), // KEY_TAB
        16 => Some(0x10), // KEY_Q
        17 => Some(0x11), // KEY_W
        18 => Some(0x12), // KEY_E
        19 => Some(0x13), // KEY_R
        20 => Some(0x14), // KEY_T
        21 => Some(0x15), // KEY_Y
        22 => Some(0x16), // KEY_U
        23 => Some(0x17), // KEY_I
        24 => Some(0x18), // KEY_O
        25 => Some(0x19), // KEY_P
        26 => Some(0x1A), // KEY_LEFTBRACE
        27 => Some(0x1B), // KEY_RIGHTBRACE
        43 => Some(0x2B), // KEY_BACKSLASH

        // Caps Lock and home row
        58 => Some(0x3A), // KEY_CAPSLOCK
        30 => Some(0x1E), // KEY_A
        31 => Some(0x1F), // KEY_S
        32 => Some(0x20), // KEY_D
        33 => Some(0x21), // KEY_F
        34 => Some(0x22), // KEY_G
        35 => Some(0x23), // KEY_H
        36 => Some(0x24), // KEY_J
        37 => Some(0x25), // KEY_K
        38 => Some(0x26), // KEY_L
        39 => Some(0x27), // KEY_SEMICOLON
        40 => Some(0x28), // KEY_APOSTROPHE
        28 => Some(0x1C), // KEY_ENTER

        // Shift and bottom row
        42 => Some(0x2A), // KEY_LEFTSHIFT
        44 => Some(0x2C), // KEY_Z
        45 => Some(0x2D), // KEY_X
        46 => Some(0x2E), // KEY_C
        47 => Some(0x2F), // KEY_V
        48 => Some(0x30), // KEY_B
        49 => Some(0x31), // KEY_N
        50 => Some(0x32), // KEY_M
        51 => Some(0x33), // KEY_COMMA
        52 => Some(0x34), // KEY_DOT
        53 => Some(0x35), // KEY_SLASH
        54 => Some(0x36), // KEY_RIGHTSHIFT

        // Control and Alt (left side)
        29 => Some(0x1D), // KEY_LEFTCTRL
        56 => Some(0x38), // KEY_LEFTALT

        // Space bar
        57 => Some(0x39), // KEY_SPACE

        // Grave/tilde (backtick)
        41 => Some(0x29), // KEY_GRAVE

        // Scroll Lock, Num Lock
        70 => Some(0x46), // KEY_SCROLLLOCK
        69 => Some(0x45), // KEY_NUMLOCK

        // Keypad (numeric mode)
        71 => Some(0x47), // KEY_KP7
        72 => Some(0x48), // KEY_KP8
        73 => Some(0x49), // KEY_KP9
        74 => Some(0x4A), // KEY_KPMINUS
        75 => Some(0x4B), // KEY_KP4
        76 => Some(0x4C), // KEY_KP5
        77 => Some(0x4D), // KEY_KP6
        78 => Some(0x4E), // KEY_KPPLUS
        79 => Some(0x4F), // KEY_KP1
        80 => Some(0x50), // KEY_KP2
        81 => Some(0x51), // KEY_KP3
        82 => Some(0x52), // KEY_KP0
        83 => Some(0x53), // KEY_KPDOT
        55 => Some(0x37), // KEY_KPASTERISK

        // International key (102nd key on ISO keyboards)
        86 => Some(0x56), // KEY_102ND

        _ => None,
    }
}

/// Checks if a scancode represents an extended key.
///
/// Extended keys have the 0xE0 prefix in their high byte.
/// Used in tests to verify correct scancode generation for extended keys
/// like arrow keys, navigation cluster, and Windows keys.
#[inline]
pub fn is_extended_scancode(scancode: u16) -> bool {
    (scancode & 0xFF00) == 0xE000
}

/// Gets the base scancode without the extended prefix.
///
/// Extracts the low byte from a scancode, removing the 0xE0 prefix if present.
/// Useful for debugging and comparing scancodes.
#[inline]
pub fn base_scancode(scancode: u16) -> u8 {
    (scancode & 0xFF) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_letter_keys() {
        // A-Z should map correctly
        assert_eq!(wayland_to_rdp_scancode(30), Some(0x1E)); // A
        assert_eq!(wayland_to_rdp_scancode(48), Some(0x30)); // B
        assert_eq!(wayland_to_rdp_scancode(46), Some(0x2E)); // C
        assert_eq!(wayland_to_rdp_scancode(44), Some(0x2C)); // Z
    }

    #[test]
    fn test_number_keys() {
        // 0-9 should map correctly
        assert_eq!(wayland_to_rdp_scancode(11), Some(0x0B)); // 0
        assert_eq!(wayland_to_rdp_scancode(2), Some(0x02)); // 1
        assert_eq!(wayland_to_rdp_scancode(10), Some(0x0A)); // 9
    }

    #[test]
    fn test_function_keys() {
        assert_eq!(wayland_to_rdp_scancode(59), Some(0x3B)); // F1
        assert_eq!(wayland_to_rdp_scancode(60), Some(0x3C)); // F2
        assert_eq!(wayland_to_rdp_scancode(68), Some(0x44)); // F10
        assert_eq!(wayland_to_rdp_scancode(87), Some(0x57)); // F11
        assert_eq!(wayland_to_rdp_scancode(88), Some(0x58)); // F12
    }

    #[test]
    fn test_extended_function_keys() {
        // F13-F15
        assert_eq!(wayland_to_rdp_scancode(183), Some(0x64)); // F13
        assert_eq!(wayland_to_rdp_scancode(184), Some(0x65)); // F14
        assert_eq!(wayland_to_rdp_scancode(185), Some(0x66)); // F15
        // F16-F24
        assert_eq!(wayland_to_rdp_scancode(186), Some(0x67)); // F16
        assert_eq!(wayland_to_rdp_scancode(187), Some(0x68)); // F17
        assert_eq!(wayland_to_rdp_scancode(188), Some(0x69)); // F18
        assert_eq!(wayland_to_rdp_scancode(189), Some(0x6A)); // F19
        assert_eq!(wayland_to_rdp_scancode(190), Some(0x6B)); // F20
        assert_eq!(wayland_to_rdp_scancode(191), Some(0x6C)); // F21
        assert_eq!(wayland_to_rdp_scancode(192), Some(0x6D)); // F22
        assert_eq!(wayland_to_rdp_scancode(193), Some(0x6E)); // F23
        assert_eq!(wayland_to_rdp_scancode(194), Some(0x6F)); // F24
    }

    #[test]
    fn test_modifier_keys() {
        assert_eq!(wayland_to_rdp_scancode(29), Some(0x1D)); // Left Ctrl
        assert_eq!(wayland_to_rdp_scancode(56), Some(0x38)); // Left Alt
        assert_eq!(wayland_to_rdp_scancode(42), Some(0x2A)); // Left Shift
        assert_eq!(wayland_to_rdp_scancode(54), Some(0x36)); // Right Shift
        assert_eq!(wayland_to_rdp_scancode(97), Some(0xE01D)); // Right Ctrl (extended)
        assert_eq!(wayland_to_rdp_scancode(100), Some(0xE038)); // Right Alt (extended)
    }

    #[test]
    fn test_arrow_keys_are_extended() {
        let up = wayland_to_rdp_scancode(103).unwrap();
        let down = wayland_to_rdp_scancode(108).unwrap();
        let left = wayland_to_rdp_scancode(105).unwrap();
        let right = wayland_to_rdp_scancode(106).unwrap();

        assert!(is_extended_scancode(up));
        assert!(is_extended_scancode(down));
        assert!(is_extended_scancode(left));
        assert!(is_extended_scancode(right));

        assert_eq!(up, 0xE048);
        assert_eq!(down, 0xE050);
        assert_eq!(left, 0xE04B);
        assert_eq!(right, 0xE04D);
    }

    #[test]
    fn test_navigation_keys_are_extended() {
        assert_eq!(wayland_to_rdp_scancode(102), Some(0xE047)); // Home
        assert_eq!(wayland_to_rdp_scancode(107), Some(0xE04F)); // End
        assert_eq!(wayland_to_rdp_scancode(104), Some(0xE049)); // Page Up
        assert_eq!(wayland_to_rdp_scancode(109), Some(0xE051)); // Page Down
        assert_eq!(wayland_to_rdp_scancode(110), Some(0xE052)); // Insert
        assert_eq!(wayland_to_rdp_scancode(111), Some(0xE053)); // Delete
    }

    #[test]
    fn test_common_keys() {
        assert_eq!(wayland_to_rdp_scancode(1), Some(0x01)); // Escape
        assert_eq!(wayland_to_rdp_scancode(14), Some(0x0E)); // Backspace
        assert_eq!(wayland_to_rdp_scancode(15), Some(0x0F)); // Tab
        assert_eq!(wayland_to_rdp_scancode(28), Some(0x1C)); // Enter
        assert_eq!(wayland_to_rdp_scancode(57), Some(0x39)); // Space
    }

    #[test]
    fn test_unknown_keycode() {
        // Very high keycodes should return None
        assert_eq!(wayland_to_rdp_scancode(999), None);
        assert_eq!(wayland_to_rdp_scancode(500), None);
    }

    #[test]
    fn test_is_extended_scancode() {
        assert!(!is_extended_scancode(0x1E)); // A - not extended
        assert!(is_extended_scancode(0xE048)); // Up arrow - extended
        assert!(!is_extended_scancode(0x0048)); // Not extended (wrong prefix)
    }

    #[test]
    fn test_base_scancode() {
        assert_eq!(base_scancode(0x1E), 0x1E);
        assert_eq!(base_scancode(0xE048), 0x48);
        assert_eq!(base_scancode(0xE01D), 0x1D);
    }

    #[test]
    fn test_windows_keys() {
        // Windows/Super keys are extended
        assert_eq!(wayland_to_rdp_scancode(125), Some(0xE05B)); // Left Windows
        assert_eq!(wayland_to_rdp_scancode(126), Some(0xE05C)); // Right Windows
        assert_eq!(wayland_to_rdp_scancode(127), Some(0xE05D)); // Menu/Apps key

        // Verify they are extended
        assert!(is_extended_scancode(0xE05B));
        assert!(is_extended_scancode(0xE05C));
        assert!(is_extended_scancode(0xE05D));
    }

    #[test]
    fn test_print_screen_and_pause() {
        // Print Screen (SysRq) - extended
        assert_eq!(wayland_to_rdp_scancode(99), Some(0xE037));
        assert!(is_extended_scancode(0xE037));

        // Pause/Break - extended (simplified mapping)
        assert_eq!(wayland_to_rdp_scancode(119), Some(0xE046));
        assert!(is_extended_scancode(0xE046));
    }

    #[test]
    fn test_lock_keys() {
        // Lock keys are NOT extended (standard scancodes)
        assert_eq!(wayland_to_rdp_scancode(58), Some(0x3A)); // Caps Lock
        assert_eq!(wayland_to_rdp_scancode(69), Some(0x45)); // Num Lock
        assert_eq!(wayland_to_rdp_scancode(70), Some(0x46)); // Scroll Lock

        assert!(!is_extended_scancode(0x3A));
        assert!(!is_extended_scancode(0x45));
        assert!(!is_extended_scancode(0x46));
    }

    #[test]
    fn test_keypad_special_keys() {
        // Keypad Enter and Keypad / are extended
        assert_eq!(wayland_to_rdp_scancode(96), Some(0xE01C)); // Keypad Enter
        assert_eq!(wayland_to_rdp_scancode(98), Some(0xE035)); // Keypad /

        assert!(is_extended_scancode(0xE01C));
        assert!(is_extended_scancode(0xE035));
    }
}
