/// Green
const GOOD_COLOR: Color = Color {
    red: 76,
    green: 175,
    blue: 80,
    opacity: 255,
};
/// Yellow
const MEDIUM_COLOR: Color = Color {
    red: 255,
    green: 235,
    blue: 59,
    opacity: 255,
};
/// Red
pub(crate) const BAD_COLOR: Color = Color {
    red: 244,
    green: 67,
    blue: 73,
    opacity: 255,
};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Color {
    red: u32,
    green: u32,
    blue: u32,
    opacity: u32,
}

impl Color {
    #[cfg(test)]
    pub(crate) fn new(red: u32, green: u32, blue: u32, opacity: u32) -> Self {
        Color {
            red,
            green,
            blue,
            opacity,
        }
    }

    /// linear interpolation between two colors and opacities
    fn lerp(start: Color, end: Color, fraction: f64) -> Color {
        let red = start.red as f64 + (end.red as f64 - start.red as f64) * fraction;
        let green = start.green as f64 + (end.green as f64 - start.green as f64) * fraction;
        let blue = start.blue as f64 + (end.blue as f64 - start.blue as f64) * fraction;
        let opacity = start.opacity as f64 + (end.opacity as f64 - start.opacity as f64) * fraction;

        Color {
            red: red.round() as u32,
            green: green.round() as u32,
            blue: blue.round() as u32,
            opacity: opacity.round() as u32,
        }
    }

    pub(crate) fn argb(&self) -> u32 {
        ((self.opacity) << 24) | ((self.red) << 16) | ((self.green) << 8) | (self.blue)
    }
}

pub(crate) fn percent_to_color(fraction: f64) -> Color {
    if fraction <= 0.5 {
        let scaled_fraction = fraction * 2.0;
        Color::lerp(GOOD_COLOR, MEDIUM_COLOR, scaled_fraction)
    } else {
        let scaled_fraction = (fraction - 0.5) * 2.0;
        Color::lerp(MEDIUM_COLOR, BAD_COLOR, scaled_fraction)
    }
}
