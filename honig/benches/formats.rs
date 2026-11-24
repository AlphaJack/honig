//! String `format` checks that the server runs on every value of a request. One value each as a field of a body
//! contains it. A check costs per value and not per body byte.

use divan::Bencher;
use honig::__private::check_format;
use honig::{EmailStr, HostnameStr, IdnEmailStr, IdnHostnameStr, InstanceLocation};

fn main() {
    // Without its honig feature a format is not checked. Its bench would then time nothing.
    for (format, check) in CASES {
        assert!(!check("-x-@"), "{format} is not checked: turn on its honig feature");
    }
    divan::Divan::default().sample_count(50).max_time(std::time::Duration::from_millis(200)).config_with_args().main();
}

/// A `String` that the crate has no library type for. It is checked as such.
fn text(format: &str, value: &str) -> bool {
    let mut out = Vec::new();
    check_format(&mut out, &InstanceLocation::Root, value, format);
    out.is_empty()
}

/// How a value of a format is checked.
type Check = fn(&str) -> bool;

/// A format and how a value of it is checked: the text checks of honig, those that `url` and `ipnet` add, and the
/// types of mail addresses and host names (`idn`).
const CASES: [(&str, Check); 8] = [
    ("uuid", |v| text("uuid", v)),
    ("date-time", |v| text("date-time", v)),
    ("email", |v| EmailStr::try_from(v).is_ok()),
    ("idn-email", |v| IdnEmailStr::try_from(v).is_ok()),
    ("hostname", |v| HostnameStr::try_from(v).is_ok()),
    ("idn-hostname", |v| IdnHostnameStr::try_from(v).is_ok()),
    ("uri", |v| text("uri", v)),
    ("ipv6-cidr", |v| text("ipv6-cidr", v)),
];

/// An accepted value of each format.
const VALUES: [&str; 8] = [
    "6f9619ff-8b86-d011-b42d-00c04fc964ff",
    "2026-11-12T10:28:00.123Z",
    "joe.bloggs@example.com",
    "실례@실례.테스트",
    "api.eu-west-1.example.com",
    "bücher.example",
    "https://api.example.com/v1/orders?limit=5",
    "2001:db8::/32",
];

const INDEX: [usize; 8] = [0, 1, 2, 3, 4, 5, 6, 7];

#[divan::bench(args = INDEX)]
fn check(b: Bencher<'_, '_>, i: usize) {
    let ((format, check), value) = (CASES[i], VALUES[i]);
    b.bench_local(|| assert!(check(divan::black_box(value)), "{format}: {value}"));
}
