//! Made-up mouse and keyboard for the end-to-end tests (`--headless` only): `infinitydesk msg
//! input '<event>'` acts like a real device.

use std::path::PathBuf;

use smithay::{
    backend::input::{
        Axis, AxisRelativeDirection, AxisSource, ButtonState, Device, DeviceCapability, Event,
        InputBackend, InputEvent, InputTime, KeyState, KeyboardKeyEvent, Keycode, PointerAxisEvent,
        PointerButtonEvent, PointerMotionEvent, UnusedEvent,
    },
    utils::{Logical, Point},
};

use crate::InfinityDesk;

impl InfinityDesk {
    /// One or more events, separated by `;`:
    /// - `move <dx> <dy>` (pixels on the screen), `scroll <x> <y>` (notches)
    /// - `button <left|right> <down|up>`, `key <code> <down|up>` (linux/input-event-codes.h)
    pub fn test_input(&mut self, events: &str) -> Result<String, String> {
        for text in events.split(';') {
            let event = parse(text).ok_or_else(|| format!("unknown input: {text}"))?;
            self.process_input_event(event);
        }
        Ok(String::new())
    }
}

fn parse(text: &str) -> Option<InputEvent<TestInput>> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let point = |x: &str, y: &str| Some(Point::from((x.parse().ok()?, y.parse().ok()?)));
    let down = |word: &str| match word {
        "down" => Some(true),
        "up" => Some(false),
        _ => None,
    };
    Some(match words[..] {
        ["move", x, y] => InputEvent::PointerMotion {
            event: Motion(point(x, y)?),
        },
        ["scroll", x, y] => InputEvent::PointerAxis {
            event: Scroll(point(x, y)?),
        },
        ["button", button, state] => InputEvent::PointerButton {
            event: Button {
                code: match button {
                    "left" => 0x110,
                    "right" => 0x111,
                    _ => return None,
                },
                down: down(state)?,
            },
        },
        ["key", code, state] => InputEvent::Keyboard {
            event: Key {
                code: code.parse().ok()?,
                down: down(state)?,
            },
        },
        _ => return None,
    })
}

struct TestInput;

impl InputBackend for TestInput {
    type Device = TestDevice;
    type KeyboardKeyEvent = Key;
    type PointerAxisEvent = Scroll;
    type PointerButtonEvent = Button;
    type PointerMotionEvent = Motion;
    type PointerMotionAbsoluteEvent = UnusedEvent;
    type GestureSwipeBeginEvent = UnusedEvent;
    type GestureSwipeUpdateEvent = UnusedEvent;
    type GestureSwipeEndEvent = UnusedEvent;
    type GesturePinchBeginEvent = UnusedEvent;
    type GesturePinchUpdateEvent = UnusedEvent;
    type GesturePinchEndEvent = UnusedEvent;
    type GestureHoldBeginEvent = UnusedEvent;
    type GestureHoldEndEvent = UnusedEvent;
    type TouchDownEvent = UnusedEvent;
    type TouchUpEvent = UnusedEvent;
    type TouchMotionEvent = UnusedEvent;
    type TouchCancelEvent = UnusedEvent;
    type TouchFrameEvent = UnusedEvent;
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;
    type SwitchToggleEvent = UnusedEvent;
    type SpecialEvent = ();
}

#[derive(PartialEq, Eq, Hash)]
struct TestDevice;

impl Device for TestDevice {
    fn id(&self) -> String {
        "test".into()
    }

    fn name(&self) -> String {
        "test".into()
    }

    fn has_capability(&self, capability: DeviceCapability) -> bool {
        matches!(
            capability,
            DeviceCapability::Keyboard | DeviceCapability::Pointer
        )
    }

    fn usb_id(&self) -> Option<(u32, u32)> {
        None
    }

    fn syspath(&self) -> Option<PathBuf> {
        None
    }
}

struct Motion(Point<f64, Logical>);
struct Scroll(Point<f64, Logical>);
struct Button {
    code: u32,
    down: bool,
}
struct Key {
    code: u32,
    down: bool,
}

macro_rules! event {
    ($($name:ident)*) => {$(
        impl Event<TestInput> for $name {
            fn time(&self) -> InputTime {
                InputTime::now()
            }

            fn device(&self) -> TestDevice {
                TestDevice
            }
        }
    )*};
}
event!(Motion Scroll Button Key);

impl PointerMotionEvent<TestInput> for Motion {
    fn delta_x(&self) -> f64 {
        self.0.x
    }

    fn delta_y(&self) -> f64 {
        self.0.y
    }

    fn delta_x_unaccel(&self) -> f64 {
        self.0.x
    }

    fn delta_y_unaccel(&self) -> f64 {
        self.0.y
    }
}

impl PointerAxisEvent<TestInput> for Scroll {
    fn amount(&self, _axis: Axis) -> Option<f64> {
        None
    }

    fn amount_v120(&self, axis: Axis) -> Option<f64> {
        Some(match axis {
            Axis::Horizontal => self.0.x * 120.0,
            Axis::Vertical => self.0.y * 120.0,
        })
    }

    fn source(&self) -> AxisSource {
        AxisSource::Wheel
    }

    fn relative_direction(&self, _axis: Axis) -> AxisRelativeDirection {
        AxisRelativeDirection::Identical
    }
}

impl PointerButtonEvent<TestInput> for Button {
    fn button_code(&self) -> u32 {
        self.code
    }

    fn state(&self) -> ButtonState {
        match self.down {
            true => ButtonState::Pressed,
            false => ButtonState::Released,
        }
    }
}

impl KeyboardKeyEvent<TestInput> for Key {
    fn key_code(&self) -> Keycode {
        // XKB counts 8 higher.
        (self.code + 8).into()
    }

    fn state(&self) -> KeyState {
        match self.down {
            true => KeyState::Pressed,
            false => KeyState::Released,
        }
    }

    fn count(&self) -> u32 {
        1
    }
}
