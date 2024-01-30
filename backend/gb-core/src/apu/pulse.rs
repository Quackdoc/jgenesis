use crate::apu::components::{Envelope, PulseTimer, StandardLengthCounter};
use bincode::{Decode, Encode};
use jgenesis_common::num::GetBit;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
enum DutyCycle {
    #[default]
    OneEighth,
    OneFourth,
    OneHalf,
    ThreeFourths,
}

impl DutyCycle {
    fn waveform_step(self, phase: u8) -> bool {
        match self {
            // 00000001
            Self::OneEighth => 0b1000_0000_u8.bit(phase),
            // 10000001
            Self::OneFourth => 0b1000_0001_u8.bit(phase),
            // 10000111
            Self::OneHalf => 0b1110_0001_u8.bit(phase),
            // 01111110
            Self::ThreeFourths => 0b0111_1110_u8.bit(phase),
        }
    }

    fn from_byte(byte: u8) -> Self {
        match (byte >> 6) & 0x03 {
            0x00 => Self::OneEighth,
            0x01 => Self::OneFourth,
            0x02 => Self::OneHalf,
            0x03 => Self::ThreeFourths,
            _ => unreachable!("value & 0x03 is always <= 0x03"),
        }
    }

    fn to_bits(self) -> u8 {
        match self {
            Self::OneEighth => 0x00,
            Self::OneFourth => 0x40,
            Self::OneHalf => 0x80,
            Self::ThreeFourths => 0xC0,
        }
    }
}

#[derive(Debug, Clone, Encode, Decode)]
struct SweepUnit {
    enabled: bool,
    shadow_frequency: u16,
    counter: u8,
    period: u8,
    shift: u8,
    negate: bool,
    calculated_with_negate_since_trigger: bool,
}

impl SweepUnit {
    fn new() -> Self {
        Self {
            enabled: false,
            shadow_frequency: 0,
            counter: 0,
            period: 0,
            shift: 0,
            negate: false,
            calculated_with_negate_since_trigger: false,
        }
    }

    fn clock(&mut self, timer: &mut PulseTimer, channel_enabled: &mut bool) {
        if !self.enabled {
            return;
        }

        self.counter -= 1;
        if self.counter == 0 {
            self.counter = self.counter_reload_value();

            if self.period == 0 {
                // Period of 0 disables sweep updates (but not the sweep unit counter; a period
                // of 0 is treated as 8 as far as the counter is concerned)
                return;
            }

            let next_frequency = self.calculate_next_frequency();
            if next_frequency <= 2047 && self.shift != 0 {
                self.shadow_frequency = next_frequency;
                timer.write_frequency(next_frequency);

                // When sweep adjusts frequency, it immediately runs another frequency calculation
                // and will disable the channel if the second calculation overflows
                if self.calculate_next_frequency() > 2047 {
                    *channel_enabled = false;
                }
            } else if next_frequency > 2047 {
                *channel_enabled = false;
            }
        }
    }

    fn calculate_next_frequency(&mut self) -> u16 {
        let mut delta = self.shadow_frequency >> self.shift;
        if self.negate {
            delta = (!delta).wrapping_add(1);
            self.calculated_with_negate_since_trigger = true;
        }

        self.shadow_frequency.wrapping_add(delta)
    }

    fn trigger(&mut self, timer: PulseTimer, channel_enabled: &mut bool) {
        self.shadow_frequency = timer.frequency();
        self.counter = self.counter_reload_value();

        self.enabled = self.period != 0 || self.shift != 0;

        self.calculated_with_negate_since_trigger = false;

        // If shift is non-zero, trigger immediately runs a frequency calculation and will disable
        // the channel if it overflows
        if self.shift != 0 && self.calculate_next_frequency() > 2047 {
            *channel_enabled = false;
        }
    }

    fn counter_reload_value(&self) -> u8 {
        if self.period == 0 { 8 } else { self.period }
    }

    fn read_register(&self) -> u8 {
        0x80 | (self.period << 4) | (u8::from(self.negate) << 3) | self.shift
    }

    fn write_register(&mut self, value: u8, channel_enabled: &mut bool) {
        self.period = (value >> 4) & 0x07;
        self.negate = value.bit(3);
        self.shift = value & 0x07;

        if self.counter == 0 {
            self.counter = self.period;
        }

        if self.calculated_with_negate_since_trigger && !self.negate {
            // If the negate flag is cleared after frequency was calculated with it set at least
            // once, the channel is immediately disabled
            *channel_enabled = false;
        }
    }
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct PulseChannel {
    duty_cycle: DutyCycle,
    length_counter: StandardLengthCounter,
    envelope: Envelope,
    sweep: SweepUnit,
    timer: PulseTimer,
    channel_enabled: bool,
    dac_enabled: bool,
}

impl PulseChannel {
    pub fn new() -> Self {
        Self {
            duty_cycle: DutyCycle::default(),
            length_counter: StandardLengthCounter::new(),
            envelope: Envelope::new(),
            sweep: SweepUnit::new(),
            timer: PulseTimer::new(),
            channel_enabled: false,
            dac_enabled: false,
        }
    }

    pub fn clock_sweep(&mut self) {
        self.sweep.clock(&mut self.timer, &mut self.channel_enabled);
    }

    pub fn clock_length_counter(&mut self) {
        self.length_counter.clock(&mut self.channel_enabled);
    }

    pub fn clock_envelope(&mut self) {
        self.envelope.clock();
    }

    pub fn tick_m_cycle(&mut self) {
        self.timer.tick_m_cycle();
    }

    pub fn sample(&self) -> Option<u8> {
        if !self.dac_enabled {
            return None;
        }

        if !self.channel_enabled {
            return Some(0);
        }

        let waveform_step = self.duty_cycle.waveform_step(self.timer.phase);
        Some(u8::from(waveform_step) * self.envelope.volume)
    }

    pub fn read_register_0(&self) -> u8 {
        self.sweep.read_register()
    }

    pub fn write_register_0(&mut self, value: u8) {
        // NR10: Pulse 1 sweep control
        self.sweep.write_register(value, &mut self.channel_enabled);

        log::trace!("NR10 write, sweep: {:?}", self.sweep);
    }

    pub fn read_register_1(&self) -> u8 {
        0x3F | self.duty_cycle.to_bits()
    }

    pub fn write_register_1(&mut self, value: u8) {
        // NR11/NR21: Pulse duty cycle and length counter reload
        self.duty_cycle = DutyCycle::from_byte(value);
        self.length_counter.load(value);

        log::trace!("NRx1 write");
        log::trace!("  Duty cycle: {:?}", self.duty_cycle);
        log::trace!("  Length counter: {}", self.length_counter.counter);
    }

    pub fn read_register_2(&self) -> u8 {
        self.envelope.read_register()
    }

    pub fn write_register_2(&mut self, value: u8) {
        // NR12/NR22: Pulse envelope control
        self.envelope.write_register(value);
        self.dac_enabled = value & 0xF8 != 0;

        if !self.dac_enabled {
            // Disabling DAC always disables the channel
            self.channel_enabled = false;
        }

        log::trace!("NRx2 write");
        log::trace!("  Envelope: {:?}", self.envelope);
        log::trace!("  DAC enabled: {}", self.dac_enabled);
    }

    pub fn write_register_3(&mut self, value: u8) {
        // NR13/NR23: Pulse frequency low bits
        self.timer.write_frequency_low(value);

        log::trace!("NRx3 write");
        log::trace!("  Timer frequency: {}", self.timer.frequency());
    }

    pub fn read_register_4(&self) -> u8 {
        0xBF | (u8::from(self.length_counter.enabled) << 6)
    }

    pub fn write_register_4(&mut self, value: u8, frame_sequencer_step: u8) {
        // NR14/NR24: Pulse frequency high bits + length counter enabled + trigger
        self.timer.write_frequency_high(value);
        self.length_counter.set_enabled(
            value.bit(6),
            frame_sequencer_step,
            &mut self.channel_enabled,
        );

        if value.bit(7) {
            // Channel triggered
            self.channel_enabled = true;

            self.length_counter.trigger(frame_sequencer_step);
            self.envelope.trigger();
            self.timer.trigger();
            self.sweep.trigger(self.timer, &mut self.channel_enabled);

            self.channel_enabled &= self.dac_enabled;
        }

        log::trace!("NRx4 write");
        log::trace!("  Timer frequency: {}", self.timer.frequency());
        log::trace!("  Length counter enabled: {}", self.length_counter.enabled);
        log::trace!("  Triggered: {}", value.bit(7));
    }

    pub fn enabled(&self) -> bool {
        self.channel_enabled
    }
}
