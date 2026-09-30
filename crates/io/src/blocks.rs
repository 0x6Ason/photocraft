//! Tagged-block helpers: the preserved block-list encoding, fills, text,
//! smart objects, locks and label colors.

use photocraft_color::{Color, ColorMode};
use photocraft_doc::{Fill, GradientStyle, LabelColor, Locks};
use photocraft_geom::Affine;
use photocraft_psd::descriptor::{Descriptor, Id, UnicodeString, Value, VersionedDescriptor};

/// `lspf` bits (Adobe spec: bit 0 transparency, 1 composite, 2 position).
/// Artboard (bit 3... here `0x10` as observed by ag-psd) and "all" (bit 31)
/// are undocumented.
pub fn locks_from_lspf(v: u32) -> Locks {
    Locks {
        transparency: v & 1 != 0,
        pixels: v & 2 != 0,
        position: v & 4 != 0,
        artboard: v & 0x10 != 0,
        all: v & 0x8000_0000 != 0,
    }
}

/// Inverse of [`locks_from_lspf`].
pub fn lspf_from_locks(l: &Locks) -> u32 {
    u32::from(l.transparency)
        | u32::from(l.pixels) << 1
        | u32::from(l.position) << 2
        | u32::from(l.artboard) << 4
        | u32::from(l.all) << 31
}

/// `lclr` index → label.
pub fn label_from_index(v: u16) -> LabelColor {
    match v {
        1 => LabelColor::Red,
        2 => LabelColor::Orange,
        3 => LabelColor::Yellow,
        4 => LabelColor::Green,
        5 => LabelColor::Blue,
        6 => LabelColor::Violet,
        7 => LabelColor::Gray,
        _ => LabelColor::None,
    }
}

/// Label → `lclr` index.
pub fn label_index(l: LabelColor) -> u16 {
    match l {
        LabelColor::None => 0,
        LabelColor::Red => 1,
        LabelColor::Orange => 2,
        LabelColor::Yellow => 3,
        LabelColor::Green => 4,
        LabelColor::Blue => 5,
        LabelColor::Violet => 6,
        LabelColor::Gray => 7,
    }
}

pub(crate) fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Double(d) => Some(*d),
        Value::UnitFloat { value, .. } => Some(*value),
        Value::Integer(i) => Some(f64::from(*i)),
        _ => None,
    }
}

/// Descriptor color (`RGBC`, `CMYC`, `Grsc`, `HSBC`) → [`Color`].
pub fn color_from_desc(d: &Descriptor) -> Option<Color> {
    let id = d.class_id.as_bytes();
    let g = |k: &str| num(d.get(k)).map(|v| v as f32);
    Some(match id {
        b"RGBC" => Color::rgb(g("Rd  ")? / 255.0, g("Grn ")? / 255.0, g("Bl  ").or_else(|| g("Bl  "))? / 255.0),
        b"CMYC" => Color {
            mode: ColorMode::Cmyk,
            c: [g("Cyn ")? / 100.0, g("Mgnt")? / 100.0, g("Ylw ")? / 100.0, g("Blck")? / 100.0],
            alpha: 1.0,
        },
        b"Grsc" => Color::gray(1.0 - g("Gry ")? / 100.0),
        // Newer Photoshop writes 0..1 floats.
        _ if d.get("redFloat").is_some() => Color::rgb(g("redFloat")?, g("greenFloat")?, g("blueFloat")?),
        _ => {
            // Fall back to any RGB-like triple.
            Color::rgb(g("Rd  ")? / 255.0, g("Grn ")? / 255.0, g("Bl  ")? / 255.0)
        }
    })
}

/// [`Color`] → descriptor in its own model.
pub fn color_to_desc(c: &Color) -> Descriptor {
    let d = |v: f32| Value::Double(f64::from(v));
    match c.mode {
        ColorMode::Cmyk => Descriptor::new("CMYC")
            .with("Cyn ", d(c.c[0] * 100.0))
            .with("Mgnt", d(c.c[1] * 100.0))
            .with("Ylw ", d(c.c[2] * 100.0))
            .with("Blck", d(c.c[3] * 100.0)),
        ColorMode::Grayscale => Descriptor::new("Grsc").with("Gry ", d((1.0 - c.c[0]) * 100.0)),
        _ => {
            let rgb = c.to_rgb();
            Descriptor::new("RGBC").with("Rd  ", d(rgb[0] * 255.0)).with("Grn ", d(rgb[1] * 255.0)).with("Bl  ", d(rgb[2] * 255.0))
        }
    }
}

pub(crate) fn get_desc<'a>(d: &'a Descriptor, key: &str) -> Option<&'a Descriptor> {
    match d.get(key)? {
        Value::Descriptor(x) | Value::GlobalObject(x) => Some(x),
        _ => None,
    }
}

/// Enumerated value of `key` (4-char or long id), if present.
pub(crate) fn enum_of<'a>(d: &'a Descriptor, key: &str) -> Option<&'a [u8]> {
    match d.get(key)? {
        Value::Enumerated { value, .. } => Some(value.as_bytes()),
        _ => None,
    }
}

pub(crate) fn bool_of(d: &Descriptor, key: &str) -> bool {
    matches!(d.get(key), Some(Value::Boolean(true)))
}

/// `GrdT` gradient type.
pub(crate) fn gradient_style(d: &Descriptor) -> GradientStyle {
    match enum_of(d, "Type") {
        Some(b"Rdl ") => GradientStyle::Radial,
        Some(b"Angl") => GradientStyle::Angle,
        Some(b"Rflc") => GradientStyle::Reflected,
        Some(b"Dmnd") => GradientStyle::Diamond,
        _ => GradientStyle::Linear,
    }
}

pub(crate) fn gradient_style_value(s: GradientStyle) -> Value {
    let v = match s {
        GradientStyle::Linear => "Lnr ",
        GradientStyle::Radial => "Rdl ",
        GradientStyle::Angle => "Angl",
        GradientStyle::Reflected => "Rflc",
        GradientStyle::Diamond => "Dmnd",
    };
    Value::Enumerated { type_id: Id::new("GrdT"), value: Id::new(v) }
}

/// Colour and opacity stops of a `Grdn` descriptor (locations 0..4096 → 0..1).
/// Colour stops and opacity stops of a gradient.
pub(crate) type Stops = (Vec<(f32, Color)>, Vec<(f32, f32)>);

/// Stops of a `Grdn` descriptor, baked to Photoshop's interpolation (smoothness `Intr`, stop
/// midpoints, and the parent's `gs99` interpolation method when given).
pub(crate) fn gradient_stops_with(grad: &Descriptor, method: Option<&[u8]>) -> Stops {
    let mut stops = Vec::new();
    let mut mids = Vec::new();
    let mut opacity = Vec::new();
    if let Some(Value::List(items)) = grad.get("Clrs") {
        for it in items {
            if let Value::Descriptor(s) = it {
                let loc = num(s.get("Lctn")).unwrap_or(0.0) as f32 / 4096.0;
                if let Some(c) = get_desc(s, "Clr ").and_then(color_from_desc) {
                    stops.push((loc, c));
                    mids.push(num(s.get("Mdpn")).unwrap_or(50.0) as f32 / 100.0);
                }
            }
        }
    }
    // Midpoint k applies to the segment after stop k.
    let mut order: Vec<usize> = (0..stops.len()).collect();
    order.sort_by(|a, b| stops[*a].0.total_cmp(&stops[*b].0));
    let mids: Vec<f32> = order.iter().skip(1).map(|i| mids[*i]).collect();
    let smooth = num(grad.get("Intr")).map_or(0.0, |v| (v / 4096.0) as f32);
    let stops = if stops.len() >= 2 { crate::gradient_bake::bake(stops, &mids, smooth, crate::gradient_bake::Method::from_code(method)) } else { stops };
    if let Some(Value::List(items)) = grad.get("Trns") {
        for it in items {
            if let Value::Descriptor(s) = it {
                let loc = num(s.get("Lctn")).unwrap_or(0.0) as f32 / 4096.0;
                opacity.push((loc, num(s.get("Opct")).unwrap_or(100.0) as f32 / 100.0));
            }
        }
    }
    (stops, opacity)
}

/// A `Grdn` descriptor for colour and opacity stops.
pub(crate) fn gradient_desc(stops: &[(f32, Color)], opacity: &[(f32, f32)]) -> Descriptor {
    let clrs = stops
        .iter()
        .map(|(t, c)| {
            Value::Descriptor(
                Descriptor::new("Clrt")
                    .with("Clr ", Value::Descriptor(color_to_desc(c)))
                    .with("Type", Value::Enumerated { type_id: Id::new("Clry"), value: Id::new("UsrS") })
                    .with("Lctn", Value::Integer((t * 4096.0).round() as i32))
                    .with("Mdpn", Value::Integer(50)),
            )
        })
        .collect();
    let op: Vec<(f32, f32)> = if opacity.is_empty() { stops.iter().map(|(t, c)| (*t, c.alpha)).collect() } else { opacity.to_vec() };
    let trns = op
        .iter()
        .map(|(t, a)| {
            Value::Descriptor(
                Descriptor::new("TrnS")
                    .with("Opct", Value::UnitFloat { unit: *b"#Prc", value: f64::from(a * 100.0) })
                    .with("Lctn", Value::Integer((t * 4096.0).round() as i32))
                    .with("Mdpn", Value::Integer(50)),
            )
        })
        .collect();
    Descriptor::new("Grdn")
        .with("Nm  ", Value::Text(UnicodeString::new_nul("Custom")))
        .with("GrdF", Value::Enumerated { type_id: Id::new("GrdF"), value: Id::new("CstS") })
        // Our stops interpolate linearly (Photoshop smoothness is baked into them on import).
        .with("Intr", Value::Double(0.0))
        .with("Clrs", Value::List(clrs))
        .with("Trns", Value::List(trns))
}

/// Parses a fill block (`SoCo`, `GdFl`, `PtFl`).
pub fn parse_fill(key: &[u8; 4], data: &[u8]) -> Option<Fill> {
    // Block data may carry trailing padding after the descriptor.
    let d = parse_prefix_versioned(data)?;
    fill_from_desc(key, &d)
}

/// A fill from its descriptor (`SoCo`/`GdFl`/`PtFl` layout; also the `solidColorLayer`,
/// `gradientLayer` and `patternLayer` contents of shape strokes).
pub fn fill_from_desc(key: &[u8; 4], d: &Descriptor) -> Option<Fill> {
    match key {
        b"SoCo" => Some(Fill::Solid(color_from_desc(get_desc(d, "Clr ")?)?)),
        b"GdFl" => {
            let angle = num(d.get("Angl")).unwrap_or(90.0) as f32;
            let scale = num(d.get("Scl ")).map_or(1.0, |v| v as f32 / 100.0);
            let (mut stops, _) = get_desc(d, "Grad").map(|g| gradient_stops_with(g, enum_of(d, "gs99"))).unwrap_or_default();
            if stops.is_empty() {
                stops = vec![(0.0, Color::BLACK), (1.0, Color::WHITE)];
            }
            Some(Fill::Gradient { stops, angle, scale, style: gradient_style(d), reverse: bool_of(d, "Rvrs") })
        }
        b"PtFl" => {
            let name = get_desc(d, "Ptrn")
                .and_then(|p| match p.get("Nm  ") {
                    Some(Value::Text(t)) => Some(t.to_string_lossy()),
                    _ => None,
                })
                .unwrap_or_default();
            let scale = num(d.get("Scl ")).map_or(1.0, |v| v as f32 / 100.0);
            Some(Fill::Pattern { name, scale })
        }
        _ => None,
    }
}

/// Serializes a fill to its block.
pub fn write_fill(f: &Fill) -> ([u8; 4], Vec<u8>) {
    let (k, d) = fill_to_desc(f);
    (k, VersionedDescriptor::new(d).to_bytes())
}

/// A fill as (block key, descriptor with class `null`).
pub fn fill_to_desc(f: &Fill) -> ([u8; 4], Descriptor) {
    match f {
        Fill::Solid(c) => (*b"SoCo", Descriptor::new("null").with("Clr ", Value::Descriptor(color_to_desc(c)))),
        Fill::Gradient { stops, angle, scale, style, reverse } => {
            let mut d = Descriptor::new("null")
                .with("Angl", Value::UnitFloat { unit: *b"#Ang", value: f64::from(*angle) })
                .with("Type", gradient_style_value(*style))
                .with("Scl ", Value::UnitFloat { unit: *b"#Prc", value: f64::from(scale * 100.0) })
                .with("Grad", Value::Descriptor(gradient_desc(stops, &[])));
            if *reverse {
                d = d.with("Rvrs", Value::Boolean(true));
            }
            (*b"GdFl", d)
        }
        Fill::Pattern { name, scale } => {
            let p = Descriptor::new("Ptrn").with("Nm  ", Value::Text(UnicodeString::new_nul(name)));
            (
                *b"PtFl",
                Descriptor::new("null")
                    .with("Scl ", Value::UnitFloat { unit: *b"#Prc", value: f64::from(scale * 100.0) })
                    .with("Ptrn", Value::Descriptor(p)),
            )
        }
    }
}

/// Text content and transform from a `TySh` block (best effort).
pub fn parse_tysh(data: &[u8]) -> Option<(String, Affine)> {
    let f = |at: usize| data.get(at..at + 8).map(|b| f64::from_be_bytes(b.try_into().unwrap_or([0; 8])));
    let mut m = [0.0; 6];
    for (i, v) in m.iter_mut().enumerate() {
        *v = f(2 + i * 8)?;
    }
    // text version (2) at 50, descriptor version (4) at 52, descriptor at 56.
    let rest = data.get(52..)?;
    let text = parse_prefix_versioned(rest).and_then(|d| match d.get("Txt ") {
        Some(Value::Text(t)) => Some(t.to_string_lossy()),
        _ => None,
    });
    Some((text.unwrap_or_default(), Affine { m }))
}

/// Parses a version-16 descriptor at the start of `data` (trailing bytes allowed).
pub(crate) fn parse_prefix_versioned(data: &[u8]) -> Option<Descriptor> {
    VersionedDescriptor::parse_prefix(data).ok().map(|(v, _)| v.descriptor)
}

/// Smart object identifier and transform from `SoLd`/`PlLd` (best effort).
pub fn parse_smart(key: &[u8; 4], data: &[u8]) -> (String, Affine) {
    // SoLd: 'soLD' + version(4) + versioned descriptor.
    let desc = if key == b"SoLd" || key == b"SoLE" {
        data.get(8..).and_then(parse_prefix_versioned)
    } else {
        None
    };
    let Some(d) = desc else { return (String::new(), Affine::IDENTITY) };
    let id = match d.get("Idnt") {
        Some(Value::Text(t)) => t.to_string_lossy(),
        _ => String::new(),
    };
    let mut affine = Affine::IDENTITY;
    if let (Some(Value::List(pts)), Some(sz)) = (d.get("Trnf"), get_desc(&d, "Sz  ")) {
        let p: Vec<f64> = pts.iter().filter_map(|v| num(Some(v))).collect();
        let (w, h) = (num(sz.get("Wdth")).unwrap_or(0.0), num(sz.get("Hght")).unwrap_or(0.0));
        if p.len() == 8 && w > 0.0 && h > 0.0 {
            affine = Affine { m: [(p[2] - p[0]) / w, (p[3] - p[1]) / w, (p[6] - p[0]) / h, (p[7] - p[1]) / h, p[0], p[1]] };
        }
    }
    (id, affine)
}

/// `masterFXSwitch` from an `lfx2` block (defaults to `true`).
pub fn effects_enabled(lfx2: &[u8]) -> bool {
    lfx2.get(4..)
        .and_then(parse_prefix_versioned)
        .and_then(|d| match d.get("masterFXSwitch") {
            Some(Value::Boolean(b)) => Some(*b),
            _ => None,
        })
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locks_and_labels() {
        for bits in [0u32, 1, 2, 4, 0x10, 0x8000_0000, 0x8000_0017] {
            assert_eq!(lspf_from_locks(&locks_from_lspf(bits)), bits);
        }
        for i in 0..8 {
            assert_eq!(label_index(label_from_index(i)), i);
        }
    }

    #[test]
    fn fills_roundtrip() {
        for f in [
            Fill::Solid(Color::rgb(1.0, 0.5, 0.0)),
            Fill::Solid(Color { mode: ColorMode::Cmyk, c: [0.1, 0.2, 0.3, 0.4], alpha: 1.0 }),
            Fill::Solid(Color::gray(0.25)),
            Fill::Gradient { stops: vec![(0.0, Color::rgb(1.0, 0.0, 0.0)), (1.0, Color::rgb(0.0, 0.0, 1.0))], angle: 45.0, scale: 1.0, style: GradientStyle::Reflected, reverse: true },
            Fill::Pattern { name: "Bubbles".into(), scale: 0.5 },
        ] {
            let (k, d) = write_fill(&f);
            let back = parse_fill(&k, &d).unwrap();
            match (&f, &back) {
                (Fill::Solid(a), Fill::Solid(b)) => {
                    assert_eq!(a.mode, b.mode);
                    for i in 0..4 {
                        assert!((a.c[i] - b.c[i]).abs() < 1e-5);
                    }
                }
                _ => assert_eq!(back, f),
            }
        }
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;

    #[test]
    fn smart_transform_from_sold() {
        let pts = [10.0, 20.0, 110.0, 20.0, 110.0, 70.0, 10.0, 70.0].map(Value::Double).to_vec();
        let d = Descriptor::new("null")
            .with("Idnt", Value::Text(UnicodeString::new_nul("uuid-1")))
            .with("Trnf", Value::List(pts))
            .with("Sz  ", Value::Descriptor(Descriptor::new("Pnt ").with("Wdth", Value::Double(50.0)).with("Hght", Value::Double(25.0))));
        let mut data = b"soLD".to_vec();
        data.extend_from_slice(&4u32.to_be_bytes());
        data.extend(VersionedDescriptor::new(d).to_bytes());
        data.extend_from_slice(&[0, 0]);
        let (id, a) = parse_smart(b"SoLd", &data);
        assert_eq!(id, "uuid-1");
        assert_eq!(a.m, [2.0, 0.0, 0.0, 2.0, 10.0, 20.0]);
        assert_eq!(parse_smart(b"PlLd", &data).0, "");
    }

    #[test]
    fn effects_switch() {
        let mut lfx = vec![0, 0, 0, 0];
        lfx.extend(VersionedDescriptor::new(Descriptor::new("null").with("masterFXSwitch", Value::Boolean(false))).to_bytes());
        assert!(!effects_enabled(&lfx));
        assert!(effects_enabled(&[0, 0, 0, 0]));
    }

    #[test]
    fn tysh_garbage_is_none() {
        assert!(parse_tysh(&[0; 10]).is_none());
    }
}
