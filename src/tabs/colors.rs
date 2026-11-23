//! Color Picker: pick any pixel on screen (`pick.rs`) and read it as HEX, RGB, HSL and OKLCH;
//! press a format to copy it.

use rune::{El, Tx, col, icon, row, text};

use super::{button, card};
use crate::icons;
use crate::island::Island;

pub struct Colors {
    /// The colour showing, as sRGB bytes.
    pub color: [u8; 3],
    /// A pick is waiting for the user's click.
    pub picking: bool,
    /// The format last copied, until the next pick.
    copied: Option<&'static str>,
}

impl Default for Colors {
    fn default() -> Self { Colors { color: [0x4c, 0x9a, 0xff], picking: false, copied: None } }
}

impl Island {
    fn colors_pick(&mut self) {
        if self.colors.picking { return }
        let Some(me) = self.me.clone() else { return };
        self.colors.picking = true;
        me.spawn(crate::pick::pick, |s, picked| {
            s.colors.picking = false;
            if let Some(c) = picked { (s.colors.color, s.colors.copied) = (c, None) }
        });
    }
}

/// sRGB byte to linear light.
fn linear(c: u8) -> f64 {
    let c = f64::from(c) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Hue in degrees, saturation and lightness in percent.
fn hsl([r, g, b]: [u8; 3]) -> (f64, f64, f64) {
    let [r, g, b] = [r, g, b].map(|c| f64::from(c) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let (l, d) = ((max + min) / 2.0, max - min);
    if d == 0.0 { return (0.0, 0.0, l * 100.0) }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r { ((g - b) / d).rem_euclid(6.0) } else if max == g { (b - r) / d + 2.0 } else { (r - g) / d + 4.0 };
    (h * 60.0, s * 100.0, l * 100.0)
}

/// Björn Ottosson's OKLab, as lightness 0..1, chroma, and hue in degrees.
fn oklch(rgb: [u8; 3]) -> (f64, f64, f64) {
    let [r, g, b] = rgb.map(linear);
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    let lightness = 0.210_454_255_3 * l + 0.793_617_785 * m - 0.004_072_046_8 * s;
    let a = 1.977_998_495_1 * l - 2.428_592_205 * m + 0.450_593_709_9 * s;
    let bb = 0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766 * s;
    let chroma = a.hypot(bb);
    // A grey has no hue; CSS reads 0 as one.
    let hue = if chroma < 2e-4 { 0.0 } else { bb.atan2(a).to_degrees().rem_euclid(360.0) };
    (lightness, chroma, hue)
}

/// The colour in each format, as CSS writes it.
fn formats(c: [u8; 3]) -> [(&'static str, String); 4] {
    let [r, g, b] = c;
    let (h, s, l) = hsl(c);
    let (ol, oc, oh) = oklch(c);
    [
        ("HEX", format!("#{r:02X}{g:02X}{b:02X}")),
        ("RGB", format!("rgb({r}, {g}, {b})")),
        // Hues round before they wrap, so 359.6 reads 0, not 360.
        ("HSL", format!("hsl({:.0}, {:.0}%, {:.0}%)", h.round() % 360.0, s, l)),
        ("OKLCH", format!("oklch({:.1}% {:.3} {:.1})", ol * 100.0, oc, (oh * 10.0).round() / 10.0 % 360.0)),
    ]
}

pub fn view(s: &Island) -> El {
    let fg = s.cfg.text();
    let muted = fg.alpha(0.55 * fg.a);
    let all = formats(s.colors.color);
    let swatch = swatch(s, &all[0].1);
    let mut list = col().key("color-formats").grow(1.0).min_w(0.0).gap(6.0);
    for (name, value) in all {
        let copied = s.colors.copied == Some(name);
        let label = format!("Copy {name} {value}");
        list = list.child(
            row().key(format!("color-{name}")).role("button").label(label).focusable(true)
                .w_full().h(36.0).px(12.0).gap(10.0).center_y().radius(s.cfg.radius(12.0)).fill(card(fg))
                .hover(fg.alpha(0.15 * fg.a)).press(fg.alpha(0.22 * fg.a))
                .child(text(name).font(s.cfg.font(12.0, 700.0)).color(muted).w(46.0).a11y_hidden())
                .child(text(value.clone()).font(s.cfg.font(14.0, 600.0).features("'tnum' 1")).color(fg).grow(1.0).min_w(0.0).truncate().a11y_hidden())
                .child(icon(if copied { &icons::CHECK } else { &icons::COPY }, 14.0).color(muted).a11y_hidden())
                .on_press_tx(move |s: &mut Island, tx: &mut Tx| { tx.copy(value.clone()); s.colors.copied = Some(name) }),
        );
    }
    let label = if s.colors.picking { "Click anywhere on screen…" } else { "Pick from screen" };
    col().key("colors").w_full().h_full().px(16.0).pt(14.0).pb(14.0).gap(12.0)
        .child(row().key("color-body").w_full().gap(12.0).child(swatch).child(list))
        .child(row().key("color-actions").w_full().gap(8.0).center_y()
            .child(icon(&icons::PIPETTE, 16.0).color(muted).a11y_hidden())
            .child(button(s, "color-pick", label, !s.colors.picking).grow(1.0).on_press(Island::colors_pick)))
}

/// The colour itself, a rounded square beside its formats.
fn swatch(s: &Island, hex: &str) -> El {
    let fg = s.cfg.text();
    // Fills are linear: decode the sRGB hex, as `accent` does.
    let fill = rune::color::hex(hex).unwrap_or(rune::Color::WHITE);
    rune::el().key("color-swatch").role("img").label(format!("Colour {hex}")).size(120.0, 162.0)
        .radius(s.cfg.radius(18.0)).fill(fill).border(1.0, fg.alpha(0.15 * fg.a))
}

#[cfg(test)]
mod tests {
    use super::formats;

    #[test]
    fn formats_match_css() {
        let red = formats([255, 0, 0]);
        assert_eq!(red.map(|f| f.1), ["#FF0000", "rgb(255, 0, 0)", "hsl(0, 100%, 50%)", "oklch(62.8% 0.258 29.2)"].map(String::from));
        assert_eq!(formats([255, 255, 255])[3].1, "oklch(100.0% 0.000 0.0)", "white is lightest and has no hue");
        assert_eq!(formats([0, 0, 0])[2].1, "hsl(0, 0%, 0%)");
        assert_eq!(formats([255, 0, 2])[2].1, "hsl(0, 100%, 50%)", "a hue just under 360 wraps to 0");
        // #4C9AFF, the default.
        let blue = formats([0x4c, 0x9a, 0xff]);
        assert_eq!(blue[2].1, "hsl(214, 100%, 65%)");
        assert_eq!(blue[3].1, "oklch(68.5% 0.168 256.0)");
    }
}
