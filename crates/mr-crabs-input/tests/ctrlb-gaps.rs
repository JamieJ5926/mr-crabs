use mr_crabs_input::{Key, KeyAction, KeyEvent, KeyboardMode, KittyFlags, Modifiers, encode_key};

fn ctrl() -> Modifiers {
    Modifiers {
        shift: false,
        alt: false,
        ctrl: true,
        super_: false,
    }
}

fn ctrl_b() -> KeyEvent {
    KeyEvent::new(Key::Character('b')).with_mods(ctrl())
}

#[test]
fn ctrl_b_legacy_is_0x02() {
    let mut out = Vec::new();
    encode_key(&ctrl_b(), &KeyboardMode::default(), &mut out);
    assert_eq!(out, vec![0x02]);
}

#[test]
fn ctrl_b_modify_other_keys_is_csi_27_5_98() {
    let mut mode = KeyboardMode::default();
    mode.modify_other_keys_2 = true;
    let mut out = Vec::new();
    encode_key(&ctrl_b(), &mode, &mut out);
    assert_eq!(out, b"\x1b[27;5;98~");
}

#[test]
fn ctrl_b_kitty_observed() {
    let mut mode = KeyboardMode::default();
    mode.kitty_flags = KittyFlags::all();
    let mut out = Vec::new();
    encode_key(&ctrl_b(), &mode, &mut out);
    assert_eq!(out, b"\x1b[98;5u");
}

#[test]
fn ctrl_b_release_suppressed_legacy() {
    let mut out = Vec::new();
    let n = encode_key(
        &ctrl_b().with_action(KeyAction::Release),
        &KeyboardMode::default(),
        &mut out,
    );
    assert_eq!(n, 0);
    assert!(out.is_empty());
}

#[test]
fn ctrl_a_and_space_question_spot_checks() {
    let mut out = Vec::new();
    encode_key(
        &KeyEvent::new(Key::Character('a')).with_mods(ctrl()),
        &KeyboardMode::default(),
        &mut out,
    );
    assert_eq!(out, vec![0x01]);

    out.clear();
    encode_key(
        &KeyEvent::new(Key::Space).with_mods(ctrl()),
        &KeyboardMode::default(),
        &mut out,
    );
    assert_eq!(out, vec![0x00]);

    out.clear();
    encode_key(
        &KeyEvent::new(Key::Character('?')).with_mods(ctrl()),
        &KeyboardMode::default(),
        &mut out,
    );
    assert_eq!(out, vec![0x7f]);
}
