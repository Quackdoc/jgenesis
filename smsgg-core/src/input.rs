use crate::num::GetBit;

#[derive(Debug, Clone, Copy, Default)]
pub struct JoypadState {
    pub up: bool,
    pub left: bool,
    pub right: bool,
    pub down: bool,
    pub button_1: bool,
    pub button_2: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PinDirection {
    Input,
    Output(bool),
}

impl PinDirection {
    fn bit(self, joypad_value: bool) -> bool {
        match self {
            Self::Input => joypad_value,
            Self::Output(output_value) => output_value,
        }
    }
}

#[derive(Debug, Clone)]
pub struct InputState {
    p1: JoypadState,
    p2: JoypadState,
    pause: bool,
    port_a_tr: PinDirection,
    port_a_th: PinDirection,
    port_b_tr: PinDirection,
    port_b_th: PinDirection,
}

impl InputState {
    pub fn new() -> Self {
        Self {
            p1: JoypadState::default(),
            p2: JoypadState::default(),
            pause: false,
            port_a_tr: PinDirection::Input,
            port_a_th: PinDirection::Input,
            port_b_tr: PinDirection::Input,
            port_b_th: PinDirection::Input,
        }
    }

    pub fn p1(&mut self) -> &mut JoypadState {
        &mut self.p1
    }

    pub fn pause_pressed(&self) -> bool {
        self.pause
    }

    pub fn set_pause(&mut self, pause: bool) {
        self.pause = pause;
    }

    pub fn write_control(&mut self, value: u8) {
        self.port_b_th = if value.bit(3) {
            PinDirection::Input
        } else {
            PinDirection::Output(value.bit(7))
        };
        self.port_b_tr = if value.bit(2) {
            PinDirection::Input
        } else {
            PinDirection::Output(value.bit(6))
        };
        self.port_a_th = if value.bit(1) {
            PinDirection::Input
        } else {
            PinDirection::Output(value.bit(5))
        };
        self.port_a_tr = if value.bit(0) {
            PinDirection::Input
        } else {
            PinDirection::Output(value.bit(4))
        };
    }

    pub fn port_dc(&self) -> u8 {
        let port_a_tr_bit = u8::from(self.port_a_tr.bit(!self.p1.button_2)) << 5;

        (u8::from(!self.p2.down) << 7)
            | (u8::from(!self.p2.up) << 6)
            | port_a_tr_bit
            | (u8::from(!self.p1.button_1) << 4)
            | (u8::from(!self.p1.right) << 3)
            | (u8::from(!self.p1.left) << 2)
            | (u8::from(!self.p1.down) << 1)
            | u8::from(!self.p1.up)
    }

    pub fn port_dd(&self) -> u8 {
        // TODO TH bits should always be 0 on a JP SMS
        let port_b_th_bit = u8::from(self.port_b_th.bit(true)) << 7;
        let port_a_th_bit = u8::from(self.port_a_th.bit(true)) << 6;
        let port_b_tr_bit = u8::from(self.port_b_tr.bit(!self.p2.button_2)) << 3;

        // TODO RESET button

        port_b_th_bit
            | port_a_th_bit
            | 0x30
            | port_b_tr_bit
            | (u8::from(!self.p2.button_1) << 2)
            | (u8::from(!self.p2.right) << 1)
            | u8::from(!self.p2.left)
    }
}