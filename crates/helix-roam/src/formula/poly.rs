//! Symbolic arithmetic, the part of Calc's algebra a table reaches: names
//! that are neither units nor constants are variables, and sums, products
//! and whole powers of them are kept as polynomials, collected and
//! expanded: `x + x` is `2 x`, `(x + 1)^2` is `x^2 + 2 x + 1`.

use std::collections::BTreeMap;

/// A product of variables, each with its power, in name order.
pub type Monomial = Vec<(String, i32)>;

/// Terms by monomial; the empty monomial is the constant.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Poly(pub BTreeMap<Monomial, f64>);

impl Poly {
    pub fn constant(value: f64) -> Poly {
        let mut terms = BTreeMap::new();
        if value != 0.0 {
            terms.insert(Vec::new(), value);
        }
        Poly(terms)
    }

    pub fn variable(name: &str) -> Poly {
        let mut terms = BTreeMap::new();
        terms.insert(vec![(name.to_string(), 1)], 1.0);
        Poly(terms)
    }

    /// The value, when there is no variable left.
    pub fn as_constant(&self) -> Option<f64> {
        match self.0.len() {
            0 => Some(0.0),
            1 => self.0.get(&Vec::new()).copied(),
            _ => None,
        }
    }

    fn tidy(mut self) -> Poly {
        self.0.retain(|_, coefficient| coefficient.abs() > 1e-12);
        self
    }

    pub fn add(&self, other: &Poly) -> Poly {
        let mut terms = self.0.clone();
        for (monomial, coefficient) in &other.0 {
            *terms.entry(monomial.clone()).or_insert(0.0) += coefficient;
        }
        Poly(terms).tidy()
    }

    pub fn scale(&self, factor: f64) -> Poly {
        Poly(
            self.0
                .iter()
                .map(|(monomial, coefficient)| (monomial.clone(), coefficient * factor))
                .collect(),
        )
        .tidy()
    }

    pub fn mul(&self, other: &Poly) -> Poly {
        let mut terms: BTreeMap<Monomial, f64> = BTreeMap::new();
        for (left, a) in &self.0 {
            for (right, b) in &other.0 {
                let mut powers: BTreeMap<String, i32> = left.iter().cloned().collect();
                for (name, power) in right {
                    *powers.entry(name.clone()).or_insert(0) += power;
                }
                let monomial: Monomial = powers.into_iter().filter(|(_, p)| *p != 0).collect();
                *terms.entry(monomial).or_insert(0.0) += a * b;
            }
        }
        Poly(terms).tidy()
    }

    pub fn pow(&self, power: u32) -> Poly {
        (0..power).fold(Poly::constant(1.0), |acc, _| acc.mul(self))
    }

    /// The derivative with respect to `name`.
    pub fn derivative(&self, name: &str) -> Poly {
        let mut terms = BTreeMap::new();
        for (monomial, coefficient) in &self.0 {
            let Some(power) = monomial
                .iter()
                .find(|(own, _)| own == name)
                .map(|(_, p)| *p)
            else {
                continue;
            };
            let lowered: Monomial = monomial
                .iter()
                .filter_map(|(own, p)| {
                    if own == name {
                        (p - 1 != 0).then(|| (own.clone(), p - 1))
                    } else {
                        Some((own.clone(), *p))
                    }
                })
                .collect();
            *terms.entry(lowered).or_insert(0.0) += coefficient * power as f64;
        }
        Poly(terms).tidy()
    }

    /// The antiderivative with respect to `name`, without a constant.
    pub fn integral(&self, name: &str) -> Poly {
        let mut terms = BTreeMap::new();
        for (monomial, coefficient) in &self.0 {
            let power = monomial
                .iter()
                .find(|(own, _)| own == name)
                .map_or(0, |(_, p)| *p);
            let mut raised: BTreeMap<String, i32> = monomial.iter().cloned().collect();
            raised.insert(name.to_string(), power + 1);
            let raised: Monomial = raised.into_iter().collect();
            *terms.entry(raised).or_insert(0.0) += coefficient / f64::from(power + 1);
        }
        Poly(terms).tidy()
    }

    /// `name` replaced by `by` everywhere.
    pub fn substitute(&self, name: &str, by: &Poly) -> Poly {
        let mut out = Poly::default();
        for (monomial, coefficient) in &self.0 {
            let mut term = Poly::constant(*coefficient);
            for (own, power) in monomial {
                let factor = if own == name {
                    by.clone()
                } else {
                    Poly::variable(own)
                };
                term = term.mul(&factor.pow(*power as u32));
            }
            out = out.add(&term);
        }
        out
    }

    /// Calc's order: highest degree first, then by name; `x^2 - 2 x y + 3`.
    pub fn display(&self, number: impl Fn(f64) -> String) -> String {
        let degree = |monomial: &Monomial| monomial.iter().map(|(_, p)| p).sum::<i32>();
        let mut terms: Vec<(&Monomial, f64)> = self.0.iter().map(|(m, c)| (m, *c)).collect();
        terms.sort_by(|(a, _), (b, _)| degree(b).cmp(&degree(a)).then(a.cmp(b)));
        let mut out = String::new();
        for (at, (monomial, coefficient)) in terms.into_iter().enumerate() {
            let negative = coefficient < 0.0;
            let magnitude = coefficient.abs();
            let factors: Vec<String> = monomial
                .iter()
                .map(|(name, power)| {
                    if *power == 1 {
                        name.clone()
                    } else {
                        format!("{name}^{power}")
                    }
                })
                .collect();
            let body = match (factors.is_empty(), magnitude == 1.0) {
                (true, _) => number(magnitude),
                (false, true) => factors.join(" "),
                (false, false) => format!("{} {}", number(magnitude), factors.join(" ")),
            };
            match (at, negative) {
                (0, true) => out.push_str(&format!("-{body}")),
                (0, false) => out.push_str(&body),
                (_, true) => out.push_str(&format!(" - {body}")),
                (_, false) => out.push_str(&format!(" + {body}")),
            }
        }
        if out.is_empty() {
            "0".to_string()
        } else {
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(poly: &Poly) -> String {
        poly.display(|n| n.to_string())
    }

    #[test]
    fn collects_expands_and_derives() {
        let x = Poly::variable("x");
        let one = Poly::constant(1.0);
        assert_eq!(show(&x.add(&x)), "2 x");
        let square = x.add(&one).pow(2);
        assert_eq!(show(&square), "x^2 + 2 x + 1");
        assert_eq!(show(&square.derivative("x")), "2 x + 2");
        assert_eq!(
            show(&square.integral("x")),
            "0.3333333333333333 x^3 + x^2 + x"
        );
        let y = Poly::variable("y");
        assert_eq!(show(&x.mul(&y).scale(-3.0).add(&one)), "-3 x y + 1");
        assert_eq!(show(&square.substitute("x", &Poly::constant(2.0))), "9");
        assert_eq!(x.add(&x.scale(-1.0)).as_constant(), Some(0.0));
    }
}
