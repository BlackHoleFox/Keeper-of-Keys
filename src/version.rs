const fn number_parse(s: &'static str) -> u16 {
    let s = s.as_bytes();
    assert!(!s.is_empty(), "version number empty");

    let mut pos = 0;
    let end = s.len();

    let mut accum = 0u16;
    let mut ever_saw_digits = false;
    while pos < end {
        let d = s[pos];
        pos += 1;
        let value = match d {
            b'0'..=b'9' => (d - b'0') as u16,
            _ => panic!("invalid digit found"),
        };

        ever_saw_digits = true;
        accum = value
    }
    if ever_saw_digits {
        accum
    } else {
        panic!("no version number available")
    }
}

impl super::AppVersion {
    pub(crate) const CURRENT: Self = Self {
        major: number_parse(env!("CARGO_PKG_VERSION_MAJOR")),
        minor: number_parse(env!("CARGO_PKG_VERSION_MINOR")),
        patch: number_parse(env!("CARGO_PKG_VERSION_PATCH")),
    };
}
