/// Zero-dependency instant calculator & unit converter for the Ferrum omnibox.
/// Never performs network calls and carefully avoids colliding with URLs or IP addresses.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalcResult {
    pub display: String,
    pub copy_value: String,
    pub subtitle: String,
}

pub fn evaluate_omnibox_calc(raw: &str) -> Option<CalcResult> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 96 {
        return None;
    }

    // 1. Try natural unit conversion ("100 km to mi", "32 c a f", "2.5 gb en mb")
    if let Some(conv) = try_unit_conversion(trimmed) {
        return Some(conv);
    }

    // 2. Try percentage expression ("15% of 240", "20% de 1500")
    if let Some(pct) = try_percentage_of(trimmed) {
        return Some(pct);
    }

    // 3. Arithmetic expression (must contain an operator, function, or leading `=`)
    let explicit = trimmed.starts_with('=');
    let expr = trimmed.strip_prefix('=').unwrap_or(trimmed).trim();
    if expr.is_empty() {
        return None;
    }

    // Avoid hijacking domains, paths, or IPv4 addresses like 127.0.0.1
    if !explicit {
        if expr.contains("://") || expr.split('.').count() > 2 {
            return None;
        }
        let has_op = expr
            .chars()
            .any(|c| matches!(c, '+' | '-' | '*' | '/' | '^' | '%' | '('))
            || expr.starts_with("sqrt")
            || expr.starts_with("abs");
        if !has_op {
            return None;
        }
        // Reject bare negative numbers or simple hyphenated words like "rust-lang"
        if !expr.chars().any(|c| c.is_ascii_digit()) {
            return None;
        }
    }

    let tokens = tokenize(expr)?;
    let mut parser = Parser {
        tokens: &tokens,
        pos: 0,
    };
    let val = parser.parse_expr()?;
    if parser.pos != tokens.len() || !val.is_finite() {
        return None;
    }
    // Avoid echoing back a single plain number unless user wrote `=`
    if !explicit && tokens.len() <= 1 {
        return None;
    }

    let formatted = format_number(val);
    Some(CalcResult {
        display: format!("= {formatted}"),
        copy_value: formatted,
        subtitle: format!("Calculadora · Clic o Enter para copiar ({expr})"),
    })
}

fn try_percentage_of(input: &str) -> Option<CalcResult> {
    let lower = input.to_ascii_lowercase();
    let (left, right) = lower
        .split_once("% of ")
        .or_else(|| lower.split_once("% de "))?;
    let pct: f64 = left.trim().replace(',', ".").parse().ok()?;
    let base: f64 = right.trim().replace(',', ".").parse().ok()?;
    let val = (pct / 100.0) * base;
    if !val.is_finite() {
        return None;
    }
    let formatted = format_number(val);
    Some(CalcResult {
        display: format!("= {formatted}"),
        copy_value: formatted,
        subtitle: format!("{pct}% de {base} · Clic o Enter para copiar"),
    })
}

fn try_unit_conversion(input: &str) -> Option<CalcResult> {
    let lower = input.to_ascii_lowercase();
    let parts: Vec<&str> = lower.split_whitespace().collect();
    if parts.len() != 4 {
        return None;
    }
    if !matches!(parts[2], "to" | "a" | "en" | "in") {
        return None;
    }
    let val: f64 = parts[0].replace(',', ".").parse().ok()?;
    let from = parts[1].trim_start_matches('°');
    let to = parts[3].trim_start_matches('°');
    if from == to {
        return None;
    }

    // Temperature
    if let Some((res, unit_lbl)) = convert_temperature(val, from, to) {
        let formatted = format_number(res);
        return Some(CalcResult {
            display: format!("{formatted} {unit_lbl}"),
            copy_value: format!("{formatted} {unit_lbl}"),
            subtitle: format!(
                "Conversor · {val} {} → {unit_lbl} · Clic para copiar",
                parts[1]
            ),
        });
    }

    // Linear unit tables: (unit_aliases, factor_to_base)
    if let Some((res, unit_lbl)) = convert_linear(val, from, to) {
        let formatted = format_number(res);
        return Some(CalcResult {
            display: format!("{formatted} {unit_lbl}"),
            copy_value: format!("{formatted} {unit_lbl}"),
            subtitle: format!("Conversor · {val} {from} → {unit_lbl} · Clic para copiar"),
        });
    }

    None
}

fn convert_temperature(val: f64, from: &str, to: &str) -> Option<(f64, &'static str)> {
    let norm = |u: &str| match u {
        "c" | "celsius" => Some(('c', "°C")),
        "f" | "fahrenheit" => Some(('f', "°F")),
        "k" | "kelvin" => Some(('k', "K")),
        _ => None,
    };
    let (fc, _) = norm(from)?;
    let (tc, lbl) = norm(to)?;
    let c = match fc {
        'c' => val,
        'f' => (val - 32.0) * 5.0 / 9.0,
        'k' => val - 273.15,
        _ => return None,
    };
    let out = match tc {
        'c' => c,
        'f' => c * 9.0 / 5.0 + 32.0,
        'k' => c + 273.15,
        _ => return None,
    };
    Some((out, lbl))
}

fn convert_linear(val: f64, from: &str, to: &str) -> Option<(f64, &'static str)> {
    // Length (meters)
    let length = |u: &str| -> Option<(f64, &'static str)> {
        match u {
            "mm" => Some((0.001, "mm")),
            "cm" => Some((0.01, "cm")),
            "m" | "metros" | "meter" | "meters" => Some((1.0, "m")),
            "km" => Some((1000.0, "km")),
            "in" | "pulgadas" | "inch" | "inches" => Some((0.0254, "in")),
            "ft" | "pies" | "foot" | "feet" => Some((0.3048, "ft")),
            "yd" | "yard" | "yards" => Some((0.9144, "yd")),
            "mi" | "millas" | "mile" | "miles" => Some((1609.344, "mi")),
            _ => None,
        }
    };
    if let (Some((f, _)), Some((t, lbl))) = (length(from), length(to)) {
        return Some((val * f / t, lbl));
    }

    // Mass (grams)
    let mass = |u: &str| -> Option<(f64, &'static str)> {
        match u {
            "mg" => Some((0.001, "mg")),
            "g" | "gr" | "gramos" => Some((1.0, "g")),
            "kg" | "kilos" => Some((1000.0, "kg")),
            "oz" | "onzas" => Some((28.349523125, "oz")),
            "lb" | "lbs" | "libras" => Some((453.59237, "lb")),
            _ => None,
        }
    };
    if let (Some((f, _)), Some((t, lbl))) = (mass(from), mass(to)) {
        return Some((val * f / t, lbl));
    }

    // Data (bytes, binary IEC-friendly 1024)
    let data = |u: &str| -> Option<(f64, &'static str)> {
        match u {
            "b" | "bytes" => Some((1.0, "B")),
            "kb" => Some((1024.0, "KB")),
            "mb" => Some((1024.0 * 1024.0, "MB")),
            "gb" => Some((1024.0 * 1024.0 * 1024.0, "GB")),
            "tb" => Some((1024.0 * 1024.0 * 1024.0 * 1024.0, "TB")),
            _ => None,
        }
    };
    if let (Some((f, _)), Some((t, lbl))) = (data(from), data(to)) {
        return Some((val * f / t, lbl));
    }

    // Speed (m/s)
    let speed = |u: &str| -> Option<(f64, &'static str)> {
        match u {
            "m/s" | "ms" => Some((1.0, "m/s")),
            "km/h" | "kmh" | "kph" => Some((1.0 / 3.6, "km/h")),
            "mph" => Some((0.44704, "mph")),
            _ => None,
        }
    };
    if let (Some((f, _)), Some((t, lbl))) = (speed(from), speed(to)) {
        return Some((val * f / t, lbl));
    }

    // Time (seconds)
    let time = |u: &str| -> Option<(f64, &'static str)> {
        match u {
            "s" | "seg" | "sec" => Some((1.0, "s")),
            "min" | "m_t" => Some((60.0, "min")),
            "h" | "hr" | "hrs" | "horas" => Some((3600.0, "h")),
            "d" | "dias" | "days" => Some((86400.0, "d")),
            _ => None,
        }
    };
    if let (Some((f, _)), Some((t, lbl))) = (time(from), time(to)) {
        return Some((val * f / t, lbl));
    }

    None
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Num(f64),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    LParen,
    RParen,
    Ident(String),
}

fn tokenize(s: &str) -> Option<Vec<Token>> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || c == '.' {
            let start = i;
            let mut dots = 0;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                if chars[i] == '.' {
                    dots += 1;
                }
                i += 1;
            }
            if dots > 1 {
                return None;
            }
            let num_str: String = chars[start..i].iter().collect();
            let v: f64 = num_str.parse().ok()?;
            out.push(Token::Num(v));
            continue;
        }
        if c.is_ascii_alphabetic() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_alphabetic() {
                i += 1;
            }
            let id: String = chars[start..i]
                .iter()
                .collect::<String>()
                .to_ascii_lowercase();
            match id.as_str() {
                "pi" => out.push(Token::Num(std::f64::consts::PI)),
                "e" => out.push(Token::Num(std::f64::consts::E)),
                "sqrt" | "abs" | "sin" | "cos" | "tan" | "round" | "floor" | "ceil" | "ln"
                | "log" => out.push(Token::Ident(id)),
                "x" => out.push(Token::Star),
                _ => return None,
            }
            continue;
        }
        match c {
            '+' => out.push(Token::Plus),
            '-' => out.push(Token::Minus),
            '*' | '×' => out.push(Token::Star),
            '/' | '÷' => out.push(Token::Slash),
            '%' => out.push(Token::Percent),
            '^' => out.push(Token::Caret),
            '(' => out.push(Token::LParen),
            ')' => out.push(Token::RParen),
            _ => return None,
        }
        i += 1;
    }
    if out.is_empty() { None } else { Some(out) }
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn parse_expr(&mut self) -> Option<f64> {
        let mut lhs = self.parse_term()?;
        while let Some(tok) = self.peek() {
            match tok {
                Token::Plus => {
                    self.pos += 1;
                    lhs += self.parse_term()?;
                }
                Token::Minus => {
                    self.pos += 1;
                    lhs -= self.parse_term()?;
                }
                _ => break,
            }
        }
        Some(lhs)
    }

    fn parse_term(&mut self) -> Option<f64> {
        let mut lhs = self.parse_power()?;
        while let Some(tok) = self.peek() {
            match tok {
                Token::Star => {
                    self.pos += 1;
                    lhs *= self.parse_power()?;
                }
                Token::Slash => {
                    self.pos += 1;
                    let rhs = self.parse_power()?;
                    if rhs == 0.0 {
                        return None;
                    }
                    lhs /= rhs;
                }
                Token::Percent => {
                    self.pos += 1;
                    let rhs = self.parse_power()?;
                    if rhs == 0.0 {
                        return None;
                    }
                    lhs %= rhs;
                }
                _ => break,
            }
        }
        Some(lhs)
    }

    fn parse_power(&mut self) -> Option<f64> {
        let base = self.parse_unary()?;
        if matches!(self.peek(), Some(Token::Caret)) {
            self.pos += 1;
            let exp = self.parse_power()?;
            return Some(base.powf(exp));
        }
        Some(base)
    }

    fn parse_unary(&mut self) -> Option<f64> {
        match self.peek() {
            Some(Token::Minus) => {
                self.pos += 1;
                Some(-self.parse_unary()?)
            }
            Some(Token::Plus) => {
                self.pos += 1;
                self.parse_unary()
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Option<f64> {
        match self.peek()?.clone() {
            Token::Num(v) => {
                self.pos += 1;
                Some(v)
            }
            Token::LParen => {
                self.pos += 1;
                let v = self.parse_expr()?;
                if !matches!(self.peek(), Some(Token::RParen)) {
                    return None;
                }
                self.pos += 1;
                Some(v)
            }
            Token::Ident(name) => {
                self.pos += 1;
                if !matches!(self.peek(), Some(Token::LParen)) {
                    return None;
                }
                self.pos += 1;
                let arg = self.parse_expr()?;
                if !matches!(self.peek(), Some(Token::RParen)) {
                    return None;
                }
                self.pos += 1;
                match name.as_str() {
                    "sqrt" => {
                        if arg < 0.0 {
                            None
                        } else {
                            Some(arg.sqrt())
                        }
                    }
                    "abs" => Some(arg.abs()),
                    "sin" => Some(arg.sin()),
                    "cos" => Some(arg.cos()),
                    "tan" => Some(arg.tan()),
                    "round" => Some(arg.round()),
                    "floor" => Some(arg.floor()),
                    "ceil" => Some(arg.ceil()),
                    "ln" => Some(arg.ln()),
                    "log" => Some(arg.log10()),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

fn format_number(val: f64) -> String {
    if (val - val.round()).abs() < 1e-9 && val.abs() < 1e15 {
        format!("{}", val.round() as i64)
    } else {
        let s = format!("{val:.6}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_arithmetic_and_precedence() {
        assert_eq!(evaluate_omnibox_calc("2 + 2 * 5").unwrap().copy_value, "12");
        assert_eq!(
            evaluate_omnibox_calc("(2 + 2) * 5").unwrap().copy_value,
            "20"
        );
        assert_eq!(evaluate_omnibox_calc("sqrt(144)").unwrap().copy_value, "12");
        assert_eq!(evaluate_omnibox_calc("2^10").unwrap().copy_value, "1024");
        assert_eq!(
            evaluate_omnibox_calc("15% de 240").unwrap().copy_value,
            "36"
        );
    }

    #[test]
    fn converts_units_and_ignores_urls() {
        assert_eq!(
            evaluate_omnibox_calc("100 c a f").unwrap().copy_value,
            "212 °F"
        );
        assert_eq!(
            evaluate_omnibox_calc("2 gb to mb").unwrap().copy_value,
            "2048 MB"
        );
        assert!(evaluate_omnibox_calc("127.0.0.1").is_none());
        assert!(evaluate_omnibox_calc("rust-lang.org").is_none());
        assert!(evaluate_omnibox_calc("42").is_none());
    }
}
