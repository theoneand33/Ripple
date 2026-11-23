//! Controls: quick toggles (Wi-Fi, Bluetooth, mute) and power actions (lock, sleep, restart,
//! shut down). Restart and Shut Down ask for a second press, as they close every app.

use rune::{Color, El, col, el, icon, row, text};

use super::{accent, card};
use crate::icons;
use crate::island::Island;
use crate::system::{self, Power, Toggle};

#[derive(Default)]
pub struct Controls {
    /// Each toggle's state, by `Toggle as usize`; None until read, or where the OS can't say.
    pub toggles: [Option<bool>; Toggle::ALL.len()],
    /// Restart or Shut Down, pressed once: the next press does it.
    pub armed: Option<Power>,
}

impl Island {
    /// Reads every toggle, as the tab (or its Overview tile) appears.
    pub fn controls_load(&mut self) {
        self.controls.armed = None;
        let Some(me) = self.me.clone() else { return };
        // One thread each: a hung tool mustn't hold up the others.
        for t in Toggle::ALL { me.spawn(move || system::toggle(t), move |s, on| s.controls.toggles[t as usize] = on) }
    }

    /// Flips a toggle at once, then shows what the OS made of it.
    fn controls_flip(&mut self, t: Toggle) {
        let Some(on) = self.controls.toggles[t as usize] else { return };
        self.controls.toggles[t as usize] = Some(!on);
        self.controls.armed = None;
        let Some(me) = self.me.clone() else { return };
        me.spawn(move || { system::set_toggle(t, !on); system::toggle(t) }, move |s, now| s.controls.toggles[t as usize] = now);
    }

    fn controls_power(&mut self, p: Power) {
        let asks = matches!(p, Power::Restart | Power::ShutDown);
        if asks && self.controls.armed != Some(p) { self.controls.armed = Some(p); return }
        self.controls.armed = None;
        system::power(p);
    }
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.55 * fg.a);
    let mut toggles = row().key("controls-toggles").w_full().gap(10.0);
    for t in Toggle::ALL {
        let (name, glyph, tint) = match t {
            Toggle::Wifi => ("Wi-Fi", &icons::WIFI, accent::blue()),
            Toggle::Bluetooth => ("Bluetooth", &icons::BLUETOOTH, accent::bluetooth()),
            Toggle::Mute => ("Mute", &icons::VOLUME_X, accent::orange()),
        };
        let state = s.controls.toggles[t as usize];
        let on = state == Some(true);
        let status = match state { Some(true) => "On", Some(false) => "Off", None => "Unavailable" };
        let ink = if on { tint } else { fg.alpha(if state.is_some() { fg.a } else { 0.4 * fg.a }) };
        toggles = toggles.child(
            col().key(format!("toggle-{name}")).role("switch").label(name).checked(on).focusable(state.is_some()).disabled(state.is_none())
                .w(0.0).grow(1.0).min_w(0.0).padding(10.0).gap(6.0).radius(s.cfg.radius(16.0))
                .fill(if on { tint.alpha(0.22) } else { card(fg) }).hover(if on { tint.alpha(0.3) } else { fg.alpha(0.14 * fg.a) }).press_scale(0.97)
                .on_press(move |s: &mut Island| s.controls_flip(t))
                .child(icon(glyph, 20.0).color(ink).a11y_hidden())
                .child(text(name).font(s.cfg.font(13.0, 700.0)).color(ink).truncate().a11y_hidden())
                .child(text(status).font(s.cfg.font(11.0, 400.0)).color(muted).truncate().a11y_hidden()),
        );
    }
    let mut actions = row().key("controls-power").w_full().gap(10.0);
    for p in [Power::Lock, Power::Sleep, Power::Restart, Power::ShutDown] {
        let (name, glyph) = match p {
            Power::Lock => ("Lock", &icons::LOCK),
            Power::Sleep => ("Sleep", &icons::MOON),
            Power::Restart => ("Restart", &icons::ROTATE_CCW),
            Power::ShutDown => ("Shut Down", &icons::POWER),
        };
        let armed = s.controls.armed == Some(p);
        let (label, ink): (&str, Color) = if armed { ("Confirm", accent::red()) } else { (name, fg) };
        actions = actions.child(
            col().key(format!("power-{name}")).role("button").label(if armed { format!("Confirm {name}") } else { name.to_string() }).focusable(true)
                .w(0.0).grow(1.0).min_w(0.0).py(10.0).gap(6.0).items_center().radius(s.cfg.radius(16.0))
                .fill(if armed { accent::red().alpha(0.22) } else { card(fg) }).hover(fg.alpha(0.14 * fg.a)).press_scale(0.97)
                .on_press(move |s: &mut Island| s.controls_power(p))
                .child(el().size(32.0, 32.0).center().radius(s.cfg.radius(16.0)).fill(ink.alpha(0.12)).a11y_hidden().child(icon(glyph, 16.0).color(ink)))
                .child(text(label).font(s.cfg.font(12.0, 600.0)).color(ink).truncate().a11y_hidden()),
        );
    }
    col().key("controls").w_full().h_full().px(16.0).pt(14.0).pb(14.0).gap(10.0).child(toggles).child(actions)
}

#[cfg(test)]
mod tests {
    use crate::config::{Config, Store, Tab};
    use crate::island::{Island, Mode};
    use crate::system::{Power, Toggle};

    #[test]
    fn restart_and_shut_down_ask_twice_and_flips_skip_unknown_toggles() {
        let mut s = Island::new(Store::memory(Config::default()), Vec::new());
        (s.mode, s.tab) = (Mode::Large, Tab::Controls);
        s.controls_power(Power::Restart);
        assert_eq!(s.controls.armed, Some(Power::Restart), "the first press only arms");
        s.controls_power(Power::ShutDown);
        assert_eq!(s.controls.armed, Some(Power::ShutDown), "another action re-arms instead of running");
        s.controls.toggles = [Some(false), None, Some(true)];
        s.controls_flip(Toggle::Bluetooth);
        s.controls_flip(Toggle::Mute);
        assert_eq!(s.controls.toggles, [Some(false), None, Some(false)], "an unknown toggle stays put; a known one flips");
        assert_eq!(s.controls.armed, None, "a toggle disarms");
    }
}
