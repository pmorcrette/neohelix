//! Physical units, the part of Calc's units a table uses: `3 m`,
//! `12 km / hr`, `uconvert($1, mi)`.
//!
//! A quantity keeps the units it was written in, so `2 km + 500 m` is
//! `2.5 km`: the right side is converted to the left's units, as Calc's
//! `usimplify` has it. Each unit is a factor and a dimension over the SI
//! base units.

/// Exponents of metre, kilogram, second, ampere, kelvin, mole, candela.
pub type Dims = [i8; 7];

const NONE: Dims = [0; 7];
const L: Dims = [1, 0, 0, 0, 0, 0, 0];
const M: Dims = [0, 1, 0, 0, 0, 0, 0];
const T: Dims = [0, 0, 1, 0, 0, 0, 0];
const I: Dims = [0, 0, 0, 1, 0, 0, 0];
const K: Dims = [0, 0, 0, 0, 1, 0, 0];
const MOL: Dims = [0, 0, 0, 0, 0, 1, 0];
const CD: Dims = [0, 0, 0, 0, 0, 0, 1];
const AREA: Dims = [2, 0, 0, 0, 0, 0, 0];
const VOLUME: Dims = [3, 0, 0, 0, 0, 0, 0];
const SPEED: Dims = [1, 0, -1, 0, 0, 0, 0];
const FORCE: Dims = [1, 1, -2, 0, 0, 0, 0];
const ENERGY: Dims = [2, 1, -2, 0, 0, 0, 0];
const POWER: Dims = [2, 1, -3, 0, 0, 0, 0];
const PRESSURE: Dims = [-1, 1, -2, 0, 0, 0, 0];
const FREQUENCY: Dims = [0, 0, -1, 0, 0, 0, 0];
const CHARGE: Dims = [0, 0, 1, 1, 0, 0, 0];
const VOLTAGE: Dims = [2, 1, -3, -1, 0, 0, 0];
const RESISTANCE: Dims = [2, 1, -3, -2, 0, 0, 0];

/// Name, factor to SI, dimension, and whether it takes a prefix (`km`).
const UNITS: &[(&str, f64, Dims, bool)] = &[
    // Length.
    ("m", 1.0, L, true),
    ("in", 0.0254, L, false),
    ("ft", 0.3048, L, false),
    ("yd", 0.9144, L, false),
    ("mi", 1609.344, L, false),
    ("nmi", 1852.0, L, false),
    ("Ang", 1e-10, L, false),
    ("au", 1.495_978_707e11, L, false),
    ("lyr", 9.460_730_472_580_8e15, L, false),
    ("pc", 3.085_677_581_491_4e16, L, true),
    // Area and volume.
    ("ha", 1e4, AREA, false),
    ("acre", 4_046.856_422_4, AREA, false),
    ("l", 1e-3, VOLUME, true),
    ("L", 1e-3, VOLUME, true),
    ("gal", 3.785_411_784e-3, VOLUME, false),
    ("qt", 9.463_529_46e-4, VOLUME, false),
    ("pt", 4.731_764_73e-4, VOLUME, false),
    ("cup", 2.365_882_365e-4, VOLUME, false),
    ("tbsp", 1.478_676_478_125e-5, VOLUME, false),
    ("tsp", 4.928_921_593_75e-6, VOLUME, false),
    ("ozfl", 2.957_352_956_25e-5, VOLUME, false),
    // Mass.
    ("g", 1e-3, M, true),
    ("t", 1e3, M, true),
    ("lb", 0.453_592_37, M, false),
    ("oz", 0.028_349_523_125, M, false),
    ("ton", 907.184_74, M, false),
    // Time. `h` is the hour here, where Calc makes it Planck's constant.
    ("s", 1.0, T, true),
    ("sec", 1.0, T, false),
    ("min", 60.0, T, false),
    ("hr", 3600.0, T, false),
    ("h", 3600.0, T, false),
    ("day", 86_400.0, T, false),
    ("wk", 604_800.0, T, false),
    ("yr", 31_557_600.0, T, false),
    // Speed.
    ("mph", 0.447_04, SPEED, false),
    ("kph", 1000.0 / 3600.0, SPEED, false),
    ("knot", 1852.0 / 3600.0, SPEED, false),
    // Mechanics.
    ("N", 1.0, FORCE, true),
    ("dyn", 1e-5, FORCE, false),
    ("lbf", 4.448_221_615_260_5, FORCE, false),
    ("J", 1.0, ENERGY, true),
    ("erg", 1e-7, ENERGY, false),
    ("cal", 4.184, ENERGY, true),
    ("Cal", 4184.0, ENERGY, false),
    ("eV", 1.602_176_634e-19, ENERGY, true),
    ("Wh", 3600.0, ENERGY, true),
    ("Btu", 1_055.055_852_62, ENERGY, false),
    ("W", 1.0, POWER, true),
    ("hp", 745.699_871_582_27, POWER, false),
    ("Pa", 1.0, PRESSURE, true),
    ("bar", 1e5, PRESSURE, true),
    ("atm", 101_325.0, PRESSURE, false),
    ("psi", 6_894.757_293_168_4, PRESSURE, false),
    ("mmHg", 133.322_387_415, PRESSURE, false),
    ("torr", 133.322_368_421, PRESSURE, false),
    ("Hz", 1.0, FREQUENCY, true),
    // Electricity.
    ("A", 1.0, I, true),
    ("C", 1.0, CHARGE, true),
    ("V", 1.0, VOLTAGE, true),
    ("ohm", 1.0, RESISTANCE, true),
    // Temperature differences, and the rest of the base units.
    ("K", 1.0, K, true),
    ("degC", 1.0, K, false),
    ("degF", 5.0 / 9.0, K, false),
    ("mol", 1.0, MOL, true),
    ("cd", 1.0, CD, false),
];

const PREFIXES: &[(&str, f64)] = &[
    ("Y", 1e24),
    ("Z", 1e21),
    ("E", 1e18),
    ("P", 1e15),
    ("T", 1e12),
    ("G", 1e9),
    ("M", 1e6),
    ("k", 1e3),
    ("h", 1e2),
    ("D", 1e1),
    ("d", 1e-1),
    ("c", 1e-2),
    ("m", 1e-3),
    ("u", 1e-6),
    ("μ", 1e-6),
    ("n", 1e-9),
    ("p", 1e-12),
    ("f", 1e-15),
    ("a", 1e-18),
];

/// A unit's factor to SI and its dimension, `km` found as `k` and `m`.
pub fn lookup(name: &str) -> Option<(f64, Dims)> {
    if let Some(&(_, factor, dims, _)) = UNITS.iter().find(|unit| unit.0 == name) {
        return Some((factor, dims));
    }
    PREFIXES.iter().find_map(|&(prefix, scale)| {
        let base = name.strip_prefix(prefix)?;
        let &(_, factor, dims, prefixed) = UNITS.iter().find(|unit| unit.0 == base)?;
        prefixed.then_some((factor * scale, dims))
    })
}

/// Units as written, each with its power: `km^1 hr^-1`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Units(pub Vec<(String, i32)>);

impl Units {
    pub fn one(name: &str) -> Units {
        Units(vec![(name.to_string(), 1)])
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The factor to SI and the dimension of the whole.
    pub fn si(&self) -> (f64, Dims) {
        let mut factor = 1.0;
        let mut dims = NONE;
        for (name, power) in &self.0 {
            let (unit_factor, unit_dims) = lookup(name).unwrap_or((1.0, NONE));
            factor *= unit_factor.powi(*power);
            for (dim, unit_dim) in dims.iter_mut().zip(unit_dims) {
                *dim += unit_dim * *power as i8;
            }
        }
        (factor, dims)
    }

    pub fn dims(&self) -> Dims {
        self.si().1
    }

    pub fn powi(&self, power: i32) -> Units {
        Units(
            self.0
                .iter()
                .map(|(name, own)| (name.clone(), own * power))
                .collect(),
        )
    }

    /// The product, with a number to multiply the value by: a unit of the
    /// right side whose dimension one on the left already has is converted
    /// to it, so `km * m` is `1000 m^2`… in `km^2`.
    pub fn times(&self, other: &Units) -> (Units, f64) {
        let mut out = self.0.clone();
        let mut scale = 1.0;
        for (name, power) in &other.0 {
            if let Some(entry) = out.iter_mut().find(|(own, _)| own == name) {
                entry.1 += power;
                continue;
            }
            let (factor, dims) = lookup(name).unwrap_or((1.0, NONE));
            let same = out.iter_mut().find(|(own, _)| {
                lookup(own).is_some_and(|(_, own_dims)| own_dims == dims && dims != NONE)
            });
            match same {
                Some(entry) => {
                    let (own_factor, _) = lookup(&entry.0).unwrap_or((1.0, NONE));
                    scale *= (factor / own_factor).powi(*power);
                    entry.1 += power;
                }
                None => out.push((name.clone(), *power)),
            }
        }
        out.retain(|(_, power)| *power != 0);
        (Units(out), scale)
    }

    /// Calc's way: `km / hr`, `kg m / s^2`, `1 / (m s)`.
    pub fn display(&self) -> String {
        let part = |(name, power): &(String, i32)| {
            if *power == 1 {
                name.clone()
            } else {
                format!("{name}^{power}")
            }
        };
        let above: Vec<String> = self
            .0
            .iter()
            .filter(|(_, power)| *power > 0)
            .map(part)
            .collect();
        let below: Vec<String> = self
            .0
            .iter()
            .filter(|(_, power)| *power < 0)
            .map(|(name, power)| part(&(name.clone(), -power)))
            .collect();
        let above = if above.is_empty() {
            "1".to_string()
        } else {
            above.join(" ")
        };
        match below.len() {
            0 => above,
            1 => format!("{above} / {}", below[0]),
            _ => format!("{above} / ({})", below.join(" ")),
        }
    }
}

/// The factor that turns a value in `from` into one in `to`, when both
/// measure the same thing.
pub fn conversion(from: &Units, to: &Units) -> Result<f64, String> {
    let (from_factor, from_dims) = from.si();
    let (to_factor, to_dims) = to.si();
    if from_dims != to_dims {
        return Err(format!(
            "{} and {} do not measure the same thing",
            describe(from),
            describe(to)
        ));
    }
    Ok(from_factor / to_factor)
}

fn describe(units: &Units) -> String {
    if units.is_empty() {
        "a plain number".to_string()
    } else {
        units.display()
    }
}

/// The SI base units of a dimension, for `ubase`.
pub fn base_units(dims: Dims) -> Units {
    const NAMES: [&str; 7] = ["m", "kg", "s", "A", "K", "mol", "cd"];
    Units(
        NAMES
            .iter()
            .zip(dims)
            .filter(|(_, power)| *power != 0)
            .map(|(name, power)| (name.to_string(), i32::from(power)))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_and_conversions() {
        assert_eq!(lookup("km"), Some((1000.0, L)));
        assert_eq!(lookup("min"), Some((60.0, T)));
        assert!(lookup("kft").is_none());
        let speed = Units(vec![("km".into(), 1), ("hr".into(), -1)]);
        assert_eq!(speed.display(), "km / hr");
        let factor = conversion(&speed, &Units::one("mph")).unwrap();
        assert!((factor - 0.621_371_192).abs() < 1e-6);
        assert!(conversion(&Units::one("m"), &Units::one("s")).is_err());
        let (area, scale) = Units::one("km").times(&Units::one("m"));
        assert_eq!((area.display(), scale), ("km^2".to_string(), 0.001));
        assert_eq!(
            Units(vec![("kg".into(), 1), ("m".into(), -1), ("s".into(), -2)]).display(),
            "kg / (m s^2)"
        );
    }
}
