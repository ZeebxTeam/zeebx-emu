//! `printf`-style formatting for BREW's `DBGPRINTF`.
//!
//! The guest passes the format string and the arguments through the AAPCS; here we only
//! translate. It is a deliberate subset — enough for what games use in logs — and any unknown
//! specifier is copied as it came, so we never lose information.

/// Where the variadic arguments come from and how to read guest strings.
pub trait ArgSource {
    /// Next 32-bit argument.
    fn next_word(&mut self) -> u32;
    /// Zero-terminated string at the given address.
    fn read_cstring(&mut self, addr: u32) -> String;
    /// Zero-terminated `AECHAR` (UTF-16) string at the given address: the `%s` of `WSPRINTF`.
    fn read_wide_string(&mut self, addr: u32) -> String;
}

/// Flags, width and precision of a specifier.
#[derive(Default)]
struct Spec {
    /// `-`: left-align.
    left: bool,
    /// `0`: pad with zeros instead of spaces.
    zero: bool,
    /// `+`: explicit sign on positives.
    plus: bool,
    /// ` `: a space in place of the sign on positives.
    space: bool,
    /// `#`: `0x` prefix on hexadecimal.
    alt: bool,
    width: usize,
    precision: Option<usize>,
}

impl Spec {
    /// Applies width and alignment to a finished field.
    fn pad(&self, body: String) -> String {
        if body.chars().count() >= self.width {
            return body;
        }
        let fill = self.width - body.chars().count();
        match self.left {
            true => body + &" ".repeat(fill),
            false => " ".repeat(fill) + &body,
        }
    }

    /// Builds a number: precision is the minimum number of digits, and zero padding only applies
    /// when there is neither precision nor left alignment — that is what C does.
    fn number(&self, sign: &str, digits: String) -> String {
        let least = self.precision.unwrap_or(0);
        let mut digits = match digits.len() < least {
            true => "0".repeat(least - digits.len()) + &digits,
            false => digits,
        };
        if self.zero && !self.left && self.precision.is_none() {
            let used = digits.len() + sign.len();
            if used < self.width {
                digits = "0".repeat(self.width - used) + &digits;
            }
        }
        self.pad(format!("{sign}{digits}"))
    }
}

/// Applies `fmt`, consuming arguments from `args`.
pub fn format(fmt: &str, args: &mut impl ArgSource) -> String {
    format_with(fmt, args, false)
}

/// `WSPRINTF`: the same format, but `%s` is an `AECHAR` string.
///
/// Read as a C string, `%s` stopped at the zero in the second byte of the first letter: Prey
/// Evil's menu formats `L"- %s -"` with the episode names, and each item came out as one letter.
pub fn format_wide(fmt: &str, args: &mut impl ArgSource) -> String {
    format_with(fmt, args, true)
}

fn format_with(fmt: &str, args: &mut impl ArgSource, wide: bool) -> String {
    let mut out = String::new();
    let mut chars = fmt.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }

        // The specifier's original text is kept so it can be returned whole if the conversion
        // at its end is one we don't know.
        let mut spec = String::from('%');
        let mut f = Spec::default();
        while let Some(&next) = chars.peek() {
            match next {
                '-' => f.left = true,
                '0' => f.zero = true,
                '+' => f.plus = true,
                ' ' => f.space = true,
                '#' => f.alt = true,
                _ => break,
            }
            spec.push(next);
            chars.next();
        }
        // Width. `*` takes it from the argument itself, and a negative width given that way is
        // the same as the `-` flag.
        if chars.peek() == Some(&'*') {
            spec.push('*');
            chars.next();
            let value = args.next_word() as i32;
            f.left |= value < 0;
            f.width = value.unsigned_abs() as usize;
        } else {
            while let Some(&next) = chars.peek() {
                let Some(digit) = next.to_digit(10) else {
                    break;
                };
                f.width = f.width * 10 + digit as usize;
                spec.push(next);
                chars.next();
            }
        }
        if chars.peek() == Some(&'.') {
            spec.push('.');
            chars.next();
            if chars.peek() == Some(&'*') {
                spec.push('*');
                chars.next();
                f.precision = Some((args.next_word() as i32).max(0) as usize);
            } else {
                let mut value = 0usize;
                while let Some(&next) = chars.peek() {
                    let Some(digit) = next.to_digit(10) else {
                        break;
                    };
                    value = value * 10 + digit as usize;
                    spec.push(next);
                    chars.next();
                }
                f.precision = Some(value);
            }
        }
        // Size modifiers. Every ARM integer arrives in 32-bit words, and `%lld` does not show up
        // in the games, so they only need to be consumed.
        while let Some(&next) = chars.peek() {
            if !"lhzjt".contains(next) {
                break;
            }
            spec.push(next);
            chars.next();
        }

        /// The sign to print on a positive number, according to the flags.
        fn positive(f: &Spec) -> &'static str {
            match (f.plus, f.space) {
                (true, _) => "+",
                (_, true) => " ",
                _ => "",
            }
        }

        match chars.next() {
            Some('%') => out.push('%'),
            Some('d') | Some('i') => {
                let value = args.next_word() as i32;
                let sign = match value < 0 {
                    true => "-",
                    false => positive(&f),
                };
                out.push_str(&f.number(sign, value.unsigned_abs().to_string()));
            }
            Some('u') => {
                let value = args.next_word();
                out.push_str(&f.number(positive(&f), value.to_string()));
            }
            Some('x') => {
                let value = args.next_word();
                let prefix = match f.alt && value != 0 {
                    true => "0x",
                    false => "",
                };
                out.push_str(&f.number(prefix, format!("{value:x}")));
            }
            Some('X') => {
                let value = args.next_word();
                let prefix = match f.alt && value != 0 {
                    true => "0X",
                    false => "",
                };
                out.push_str(&f.number(prefix, format!("{value:X}")));
            }
            Some('o') => {
                let value = args.next_word();
                out.push_str(&f.number("", format!("{value:o}")));
            }
            Some('p') => out.push_str(&f.pad(format!("{:#010x}", args.next_word()))),
            Some('c') => {
                let value = args.next_word();
                out.push_str(&f.pad(char::from_u32(value).unwrap_or('?').to_string()));
            }
            // `%S` is the other width: an `AECHAR` string in `SPRINTF`. In `WSPRINTF` the mirror
            // holds, `char`, as in Microsoft's printf — that side has not been seen in a game.
            // Without it the specifier came out as it was and consumed no argument, and the next
            // `%d` printed the pointer: the Need For Speed map said "vença 286973016 corridas".
            Some(conv @ ('s' | 'S')) => {
                let addr = args.next_word();
                let mut text = match wide ^ (conv == 'S') {
                    true => args.read_wide_string(addr),
                    false => args.read_cstring(addr),
                };
                // On a string the precision is a ceiling, not a floor: it truncates.
                if let Some(limit) = f.precision {
                    text = text.chars().take(limit).collect();
                }
                out.push_str(&f.pad(text));
            }
            // A specifier we don't know: return the original text instead of dropping it, and
            // consume no argument — consuming the wrong one would misalign everything after it.
            Some(other) => {
                out.push_str(&spec);
                out.push(other);
            }
            None => out.push_str(&spec),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        words: Vec<u32>,
        strings: Vec<(u32, &'static str)>,
        index: usize,
    }

    impl ArgSource for Fake {
        fn next_word(&mut self) -> u32 {
            let value = self.words.get(self.index).copied().unwrap_or(0);
            self.index += 1;
            value
        }

        fn read_cstring(&mut self, addr: u32) -> String {
            self.strings
                .iter()
                .find(|&&(a, _)| a == addr)
                .map(|&(_, s)| s.to_string())
                .unwrap_or_default()
        }

        // The fake keeps the strings already decoded; what the wide test checks is **which**
        // read `%s` picks, so the wide one returns the same string, tagged.
        fn read_wide_string(&mut self, addr: u32) -> String {
            format!("L:{}", self.read_cstring(addr))
        }
    }

    fn fake(words: &[u32], strings: &[(u32, &'static str)]) -> Fake {
        Fake {
            words: words.to_vec(),
            strings: strings.to_vec(),
            index: 0,
        }
    }

    #[test]
    fn formats_the_real_z_wheel_message() {
        // The Z-Wheel's first DBGPRINTF call, with the arguments it passes.
        let mut args = fake(
            &[4, 0x532c4, 0x71],
            &[(0x532c4, "..\\..\\common\\tectoy_prefsDB.c")],
        );
        assert_eq!(
            format("*dbgprintf-%d* %s:%d", &mut args),
            "*dbgprintf-4* ..\\..\\common\\tectoy_prefsDB.c:113"
        );
    }

    #[test]
    fn covers_the_basic_specifiers() {
        let mut args = fake(&[0xffff_ffff, 42, 0xbeef, 65], &[]);
        assert_eq!(format("%d %u %x %c", &mut args), "-1 42 beef A");
    }

    #[test]
    fn a_literal_percent_does_not_consume_an_argument() {
        let mut args = fake(&[7], &[]);
        assert_eq!(format("100%% de %d", &mut args), "100% de 7");
    }

    #[test]
    fn width_precision_and_alignment() {
        // Precision is the minimum number of digits, width is the field's, and `-` left-aligns.
        // Zero padding loses to precision, as in C.
        let mut args = fake(&[5], &[]);
        assert_eq!(format("[%-08.3ld]", &mut args), "[005     ]");
        let mut args = fake(&[5, 5, 5, -3i32 as u32], &[]);
        assert_eq!(format("%04d|%4d|%-4d|%+d", &mut args), "0005|   5|5   |-3");
    }

    /// Resident Evil 4 builds the stage file names with `%s_%02d.h2z`. Dropping the width gave
    /// `3d_stg02_0.h2z`, and the file on disk is `3d_stg02_00.h2z`: the game found none of the
    /// twelve stages.
    #[test]
    fn leading_zero_in_a_file_name() {
        let mut args = fake(&[0x100, 0], &[(0x100, "3d_stg02")]);
        assert_eq!(format("%s_%02d.h2z", &mut args), "3d_stg02_00.h2z");
    }

    #[test]
    fn width_from_argument_and_precision_truncates_string() {
        let mut args = fake(&[6, 42], &[]);
        assert_eq!(format("[%*d]", &mut args), "[    42]");
        let mut args = fake(&[0x200], &[(0x200, "abcdef")]);
        assert_eq!(format("[%.3s]", &mut args), "[abc]");
    }

    #[test]
    fn an_unknown_specifier_survives_without_consuming_an_argument() {
        let mut args = fake(&[9], &[]);
        assert_eq!(format("%q %d", &mut args), "%q 9");
    }

    #[test]
    fn in_wide_format_s_reads_aechar() {
        let mut args = fake(&[0x100], &[(0x100, "Escavação")]);
        assert_eq!(format_wide("- %s -", &mut args), "- L:Escavação -");
        let mut args = fake(&[0x100], &[(0x100, "Escavação")]);
        assert_eq!(format("- %s -", &mut args), "- Escavação -");
    }

    /// The Need For Speed map challenge. Without `%S`, `%d` read the pointer to the boss's name.
    #[test]
    fn uppercase_s_is_the_other_width_and_consumes_an_argument() {
        let mut args = fake(
            &[0x100, 4, 0x200],
            &[(0x100, "MARCUS"), (0x200, "MONTANHAS")],
        );
        assert_eq!(
            format("Para desafiar %S, vença %d corridas na região %S.", &mut args),
            "Para desafiar L:MARCUS, vença 4 corridas na região L:MONTANHAS."
        );
        let mut args = fake(&[0x100], &[(0x100, "MARCUS")]);
        assert_eq!(format_wide("[%S]", &mut args), "[MARCUS]");
    }

}
