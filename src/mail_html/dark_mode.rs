use enzui_iced_theme::EnzTheme;
use html5ever::{QualName, local_name, ns};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DarkPalette {
    background: [u8; 3],
    text: [u8; 3],
    link: [u8; 3],
}

impl DarkPalette {
    pub(crate) fn from_theme(theme: &EnzTheme) -> Self {
        let rgb = |color: iced::Color| {
            [color.r, color.g, color.b].map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)
        };
        Self {
            background: rgb(theme.palette.background),
            text: rgb(theme.palette.text),
            link: rgb(theme.palette.accent),
        }
    }

    pub(super) fn apply(self, doc: &mut blitz_html::HtmlDocument) {
        let nodes = doc
            .query_selector_all("html,body,body *")
            .unwrap_or_default();
        let mut original = std::collections::HashMap::new();
        for id in &nodes {
            if doc
                .get_node(*id)
                .is_none_or(|node| node.primary_styles().is_none())
            {
                continue;
            }
            let bg = color(&doc.resolved_style_value(*id, "background-color"));
            let fg = color(&doc.resolved_style_value(*id, "color"));
            let image = doc.resolved_style_value(*id, "background-image") != "none";
            original.insert(*id, (bg, fg, image));
        }
        let base = self.background.map(|v| f64::from(v) / 255.0);
        let ink = self.text.map(|v| f64::from(v) / 255.0);
        let link = self.link.map(|v| f64::from(v) / 255.0);
        let mut changes = Vec::new();
        for id in nodes {
            let Some(node) = doc.get_node(id) else {
                continue;
            };
            if ["img", "svg", "canvas", "picture", "style"]
                .iter()
                .any(|tag| super::tag(node, tag))
            {
                continue;
            }
            let Some((bg, fg, image)) = original.get(&id).copied() else {
                continue;
            };
            let mut layers = Vec::new();
            let mut parent = Some(id);
            let mut protected = false;
            for _ in 0..128 {
                let Some(p) = parent else {
                    break;
                };
                if let Some((bg, _, image)) = original.get(&p) {
                    if *image {
                        protected = true;
                        break;
                    }
                    if bg[3] > 0.0 {
                        layers.push(*bg);
                    }
                    if bg[3] >= 1.0 {
                        break;
                    }
                }
                parent = doc.get_node(p).and_then(|n| n.parent);
            }
            if protected {
                continue;
            }
            let mut before = [1.0; 3];
            let mut after = base;
            for layer in layers.into_iter().rev() {
                before = blend(before, layer);
                after = blend(after, background(layer, base));
            }
            let painted = blend(before, fg);
            let invisible = fg[3] == 0.0 || contrast(painted, before) < 1.05;
            let mut next = if super::tag(node, "a") {
                [link[0], link[1], link[2], 1.0]
            } else {
                fg
            };
            if invisible {
                next = [0.0; 4];
            } else if contrast(blend(after, next), after) < 4.5 {
                next = [ink[0], ink[1], ink[2], 1.0];
            }
            let mut attrs = node.attr("style".into()).unwrap_or_default().to_owned();
            if !image && bg[3] > 0.0 {
                attrs = doc
                    .style_attr_set_property(
                        &attrs,
                        "background-color",
                        &css(background(bg, base)),
                        true,
                    )
                    .unwrap_or(attrs);
            }
            attrs = doc
                .style_attr_set_property(&attrs, "color", &css(next), true)
                .unwrap_or(attrs);
            changes.push((id, attrs));
        }
        let mut mutator = doc.mutate();
        for (id, attrs) in changes {
            mutator.set_attribute(id, QualName::new(None, ns!(), local_name!("style")), &attrs);
        }
    }
}

fn color(value: &str) -> [f64; 4] {
    value
        .parse::<csscolorparser::Color>()
        .map_or([0.0; 4], |c| c.to_array().map(f64::from))
}
fn css(c: [f64; 4]) -> String {
    format!(
        "rgba({},{},{},{})",
        (c[0] * 255.).round(),
        (c[1] * 255.).round(),
        (c[2] * 255.).round(),
        c[3]
    )
}
fn blend(base: [f64; 3], over: [f64; 4]) -> [f64; 3] {
    std::array::from_fn(|i| over[i] * over[3] + base[i] * (1. - over[3]))
}
fn luminance(c: [f64; 3]) -> f64 {
    c.map(|v| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    })
    .iter()
    .zip([0.2126, 0.7152, 0.0722])
    .map(|(v, k)| v * k)
    .sum()
}
fn contrast(a: [f64; 3], b: [f64; 3]) -> f64 {
    let a = luminance(a);
    let b = luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
fn background(c: [f64; 4], base: [f64; 3]) -> [f64; 4] {
    let rgb = [c[0], c[1], c[2]];
    let l = luminance(rgb);
    if c[3] == 0.0 || l < 0.12 {
        return c;
    }
    let high = c[0].max(c[1]).max(c[2]);
    let low = c[0].min(c[1]).min(c[2]);
    if high - low < 24.0 / 255.0 {
        let lift = (1.0 - high) * 24.0 / 255.0;
        [base[0] + lift, base[1] + lift, base[2] + lift, c[3]]
    } else {
        let scale = (0.07 / l).sqrt().min(0.45);
        [c[0] * scale, c[1] * scale, c[2] * scale, c[3]]
    }
}
