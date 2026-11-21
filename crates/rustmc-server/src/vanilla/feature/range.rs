//! Vertical anchors and integer/height range providers.

use super::*;

/// A `VerticalAnchor`: a row expressed against one of the world bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalAnchor {
    Absolute(i32),
    AboveBottom(i32),
    BelowTop(i32),
}

impl VerticalAnchor {
    /// Resolves the anchor to an absolute row for a dimension spanning
    /// `min_y` with `height` rows.
    pub fn resolve(&self, min_y: i32, height: i32) -> i32 {
        match self {
            Self::Absolute(y) => *y,
            Self::AboveBottom(offset) => min_y + offset,
            Self::BelowTop(offset) => min_y + height - 1 - offset,
        }
    }

    /// Parses the datapack representation: either a bare number (absolute)
    /// or an object with exactly one of the three keys.
    pub(super) fn from_value(value: &Value) -> Option<Self> {
        let bounded = |number: i64| {
            i32::try_from(number)
                .ok()
                .filter(|n| (-32_768..=32_768).contains(n))
        };
        if let Some(y) = value.as_i64() {
            return Some(Self::Absolute(bounded(y)?));
        }
        let object = value.as_object()?;
        for key in ["absolute", "above_bottom", "below_top"] {
            if let Some(offset) = object.get(key).and_then(Value::as_i64)
                && object.len() == 1
            {
                return Some(match key {
                    "absolute" => Self::Absolute(bounded(offset)?),
                    "above_bottom" => Self::AboveBottom(bounded(offset)?),
                    _ => Self::BelowTop(bounded(offset)?),
                });
            }
        }
        None
    }
}

/// An integer provider over an anchor range, as used by the count and
/// height decorations. The pack expresses an ore band either as a uniform
/// range or as the symmetric triangle of a `minecraft:trapezoid`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeProvider {
    /// `minecraft:constant`, or a bare number.
    Constant(i32),
    /// `minecraft:uniform` over an inclusive anchor range.
    Uniform {
        min_inclusive: VerticalAnchor,
        max_inclusive: VerticalAnchor,
    },
    /// `minecraft:trapezoid` over an inclusive anchor range. The pack never
    /// writes the plateau or fold parameters for an ore height, so the
    /// distribution is the symmetric triangle those defaults describe.
    Trapezoid {
        min_inclusive: VerticalAnchor,
        max_inclusive: VerticalAnchor,
    },
    /// `minecraft:trapezoid` with explicit non-zero plateau width (`Java Edition 26.1`
    /// rev 3880954).
    TrapezoidPlateau {
        min_inclusive: VerticalAnchor,
        max_inclusive: VerticalAnchor,
        plateau: i32,
    },
    /// `minecraft:biased_to_bottom` (`Configured feature/int provider` rev 3267224;
    /// `Sugar Cane` rev 3899892: `11/18`, `5/18`, `2/18` over `[2..4]`).
    BiasedToBottom {
        min_inclusive: i32,
        max_inclusive: i32,
    },
    /// `minecraft:clamped` (`Configured feature/int provider` rev 3267224).
    Clamped {
        min_inclusive: i32,
        max_inclusive: i32,
        source: Box<RangeProvider>,
    },
    /// `minecraft:weighted_list` (`Configured feature/int provider` rev 3267224).
    WeightedList {
        distribution: Vec<(RangeProvider, i32)>,
    },
    /// A provider shape without a runtime here.
    Unsupported { kind: String },
}

impl RangeProvider {
    /// Parses a provider given either a constant number, a constant object,
    /// a uniform or trapezoid object, or a bare anchor object (height
    /// ranges only).
    pub(super) fn from_value(value: &Value, anchors_allowed: bool) -> Option<Self> {
        if let Some(n) = value.as_i64() {
            let n = i32::try_from(n).ok()?;
            if anchors_allowed && !(-32_768..=32_768).contains(&n) {
                return None;
            }
            return Some(Self::Constant(n));
        }
        let object = value.as_object()?;
        let anchored = |kind: &str| -> Option<Self> {
            let min_val = object
                .get("min_inclusive")
                .or_else(|| object.get("min"))
                .or_else(|| {
                    object
                        .get("value")
                        .and_then(|v| v.get("min_inclusive").or_else(|| v.get("min")))
                })?;
            let max_val = object
                .get("max_inclusive")
                .or_else(|| object.get("max"))
                .or_else(|| {
                    object
                        .get("value")
                        .and_then(|v| v.get("max_inclusive").or_else(|| v.get("max")))
                })?;
            let min_inclusive = VerticalAnchor::from_value(min_val)?;
            let max_inclusive = VerticalAnchor::from_value(max_val)?;
            // A count provider is a plain numeric range; only the height
            // decoration accepts anchors that resolve against the dimension.
            if !anchors_allowed
                && !(matches!(min_inclusive, VerticalAnchor::Absolute(_))
                    && matches!(max_inclusive, VerticalAnchor::Absolute(_)))
            {
                return None;
            }
            if kind == "minecraft:uniform" {
                Some(Self::Uniform {
                    min_inclusive,
                    max_inclusive,
                })
            } else {
                let plateau = object
                    .get("plateau")
                    .or_else(|| object.get("value").and_then(|v| v.get("plateau")))
                    .and_then(Value::as_i64)
                    .and_then(|v| i32::try_from(v).ok())
                    .unwrap_or(0);
                if plateau > 0 {
                    Some(Self::TrapezoidPlateau {
                        min_inclusive,
                        max_inclusive,
                        plateau,
                    })
                } else {
                    Some(Self::Trapezoid {
                        min_inclusive,
                        max_inclusive,
                    })
                }
            }
        };
        let plain_bounds = || -> Option<(i32, i32)> {
            let min_i = object
                .get("min_inclusive")
                .or_else(|| object.get("value").and_then(|v| v.get("min_inclusive")))
                .and_then(Value::as_i64)
                .and_then(|v| i32::try_from(v).ok())?;
            let max_i = object
                .get("max_inclusive")
                .or_else(|| object.get("value").and_then(|v| v.get("max_inclusive")))
                .and_then(Value::as_i64)
                .and_then(|v| i32::try_from(v).ok())?;
            if max_i < min_i {
                return None;
            }
            Some((min_i, max_i))
        };
        match object.get("type").and_then(Value::as_str) {
            Some("minecraft:constant") => {
                let value = object.get("value")?;
                if anchors_allowed && let Some(anchor) = VerticalAnchor::from_value(value) {
                    // A constant anchor is a degenerate range; the height
                    // decoration still resolves it through the provider.
                    return Some(Self::Uniform {
                        min_inclusive: anchor,
                        max_inclusive: anchor,
                    });
                }
                let n = i32::try_from(value.as_i64()?).ok()?;
                if anchors_allowed && !(-32_768..=32_768).contains(&n) {
                    return None;
                }
                Some(Self::Constant(n))
            }
            Some(kind @ ("minecraft:uniform" | "minecraft:trapezoid")) => anchored(kind),
            Some("minecraft:biased_to_bottom") if !anchors_allowed => {
                let (min_inclusive, max_inclusive) = plain_bounds()?;
                Some(Self::BiasedToBottom {
                    min_inclusive,
                    max_inclusive,
                })
            }
            Some("minecraft:clamped") if !anchors_allowed => {
                let (min_inclusive, max_inclusive) = plain_bounds()?;
                let src_val = object
                    .get("source")
                    .or_else(|| object.get("value").and_then(|v| v.get("source")))?;
                let source = Self::from_value(src_val, false)?;
                Some(Self::Clamped {
                    min_inclusive,
                    max_inclusive,
                    source: Box::new(source),
                })
            }
            Some("minecraft:weighted_list") if !anchors_allowed => {
                let dist_val = object
                    .get("distribution")
                    .or_else(|| object.get("value").and_then(|v| v.get("distribution")))?
                    .as_array()?;
                let mut distribution = Vec::with_capacity(dist_val.len());
                for entry in dist_val {
                    let weight = entry
                        .get("weight")
                        .and_then(Value::as_i64)
                        .and_then(|w| i32::try_from(w).ok())?;
                    if weight <= 0 {
                        return None;
                    }
                    let data = Self::from_value(entry.get("data")?, false)?;
                    distribution.push((data, weight));
                }
                if distribution.is_empty() {
                    return None;
                }
                Some(Self::WeightedList { distribution })
            }
            Some(other) => Some(Self::Unsupported {
                kind: other.to_string(),
            }),
            // No `type` key: an inline anchor pair, which only the height
            // decoration accepts.
            None if anchors_allowed => anchored("minecraft:uniform"),
            None => None,
        }
    }

    /// Whether a provider shape has a runtime here.
    pub fn is_modelable(&self) -> bool {
        match self {
            Self::Clamped { source, .. } => source.is_modelable(),
            Self::WeightedList { distribution } => {
                !distribution.is_empty()
                    && distribution
                        .iter()
                        .all(|(provider, weight)| *weight > 0 && provider.is_modelable())
            }
            Self::Unsupported { .. } => false,
            _ => true,
        }
    }

    /// Whether sampling this range provider consumes zero RNG draws.
    pub(super) fn costs_no_draws(&self) -> bool {
        matches!(self, Self::Constant(_))
    }
}

/// The vertical band of one decoration attempt, resolved to absolute rows.
impl RangeProvider {
    /// The provider's inclusive numeric range for a dimension spanning
    /// `min_y` with `height` rows, or `None` for a shape with no runtime
    /// here.
    pub fn range(&self, min_y: i32, height: i32) -> Option<(i32, i32)> {
        match self {
            Self::Constant(value) => Some((*value, *value)),
            Self::Uniform {
                min_inclusive,
                max_inclusive,
            }
            | Self::Trapezoid {
                min_inclusive,
                max_inclusive,
            }
            | Self::TrapezoidPlateau {
                min_inclusive,
                max_inclusive,
                ..
            } => Some((
                min_inclusive.resolve(min_y, height),
                max_inclusive.resolve(min_y, height),
            )),
            Self::BiasedToBottom {
                min_inclusive,
                max_inclusive,
            } => Some((*min_inclusive, *max_inclusive)),
            Self::Clamped {
                min_inclusive,
                max_inclusive,
                source,
            } => {
                let (s_min, s_max) = source.range(min_y, height)?;
                Some((
                    s_min.clamp(*min_inclusive, *max_inclusive),
                    s_max.clamp(*min_inclusive, *max_inclusive),
                ))
            }
            Self::WeightedList { distribution } => {
                let mut low: Option<i32> = None;
                let mut high: Option<i32> = None;
                for (entry, _) in distribution {
                    let (a, b) = entry.range(min_y, height)?;
                    low = Some(low.map_or(a, |prev| prev.min(a)));
                    high = Some(high.map_or(b, |prev| prev.max(b)));
                }
                low.zip(high)
            }
            Self::Unsupported { .. } => None,
        }
    }

    /// Draws the provider's value from the decoration stream.
    ///
    /// The draw count is part of the contract: a constant costs nothing, a
    /// uniform costs exactly one bounded draw even when its bounds are equal
    /// (`randomBetweenInclusive` always calls `nextInt`), and a trapezoid
    /// costs two unless its bounds are inverted. A provider whose minimum
    /// resolves above its maximum returns the minimum without drawing, which
    /// is what the reference runtime logs and does.
    pub fn sample(&self, min_y: i32, height: i32, random: &mut DecorationRandom) -> i32 {
        let Some((min, max)) = self.range(min_y, height) else {
            return 0;
        };
        if max < min {
            return min;
        }
        match self {
            Self::Constant(value) => *value,
            Self::Trapezoid { .. } => {
                let span = max - min;
                if span <= 0 {
                    return min + random.next_int_bounded(span + 1);
                }
                // `plateau` defaults to zero, so the triangle is the sum of
                // two uniforms over the halves of the span.
                let low = span / 2;
                let high = span - low;
                min + random.next_int_bounded(high + 1) + random.next_int_bounded(low + 1)
            }
            Self::TrapezoidPlateau { plateau, .. } => {
                let span = max - min;
                if span <= 0 || *plateau >= span {
                    return min + random.next_int_bounded(span.max(0) + 1);
                }
                let low = (span - *plateau) / 2;
                let high = span - low;
                min + random.next_int_bounded(high + 1) + random.next_int_bounded(low + 1)
            }
            Self::BiasedToBottom { .. } => {
                let span = max - min;
                let bound = random.next_int_bounded(span + 1) + 1;
                min + random.next_int_bounded(bound)
            }
            Self::Clamped {
                min_inclusive,
                max_inclusive,
                source,
            } => source
                .sample(min_y, height, random)
                .clamp(*min_inclusive, *max_inclusive),
            Self::WeightedList { distribution } => {
                let total: i32 = distribution.iter().map(|(_, weight)| *weight).sum();
                if total <= 0 {
                    return 0;
                }
                let mut pick = random.next_int_bounded(total);
                for (entry, weight) in distribution {
                    if pick < *weight {
                        return entry.sample(min_y, height, random);
                    }
                    pick -= *weight;
                }
                0
            }
            _ => min + random.next_int_bounded(max - min + 1),
        }
    }
}
