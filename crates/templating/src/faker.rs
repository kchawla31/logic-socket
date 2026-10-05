//! `{% faker 'name' %}`: random test data with Postman's dynamic-variable names
//! (`{{$randomEmail}}` imports as `{% faker 'randomEmail' %}`).

const FIRST: &[&str] = &[
    "Ada", "Alan", "Grace", "Linus", "Margaret", "Dennis", "Barbara", "Ken", "Radia", "Tim",
    "Frances", "Edsger",
];
const LAST: &[&str] = &[
    "Lovelace",
    "Turing",
    "Hopper",
    "Torvalds",
    "Hamilton",
    "Ritchie",
    "Liskov",
    "Thompson",
    "Perlman",
    "Berners-Lee",
    "Allen",
    "Dijkstra",
];
const CITY: &[&str] = &[
    "Berlin",
    "Toronto",
    "Lisbon",
    "Osaka",
    "Nairobi",
    "Austin",
    "Melbourne",
    "Oslo",
    "Pune",
    "Bogotá",
];
const COUNTRY: &[(&str, &str)] = &[
    ("Germany", "DE"),
    ("Canada", "CA"),
    ("Portugal", "PT"),
    ("Japan", "JP"),
    ("Kenya", "KE"),
    ("United States", "US"),
    ("Australia", "AU"),
    ("Norway", "NO"),
    ("India", "IN"),
    ("Colombia", "CO"),
];
const STREET: &[&str] = &[
    "Main St",
    "Oak Avenue",
    "Maple Road",
    "Station Lane",
    "Harbour Way",
    "Elm Street",
];
const COLOR: &[&str] = &[
    "red", "green", "blue", "orange", "purple", "teal", "magenta", "gold",
];
const WORD: &[&str] = &[
    "alpha", "beta", "gamma", "delta", "socket", "token", "pixel", "vector", "lambda", "matrix",
    "orbit", "quartz", "nimbus", "cobalt", "ember", "fjord",
];
const LOREM: &[&str] = &[
    "lorem",
    "ipsum",
    "dolor",
    "sit",
    "amet",
    "consectetur",
    "adipiscing",
    "elit",
    "sed",
    "do",
    "eiusmod",
    "tempor",
    "incididunt",
    "ut",
    "labore",
    "et",
    "dolore",
    "magna",
    "aliqua",
];
const COMPANY: &[&str] = &[
    "Acme",
    "Globex",
    "Initech",
    "Umbrella",
    "Hooli",
    "Stark Industries",
    "Wayne Enterprises",
    "Wonka",
];
const JOB: &[&str] = &[
    "Engineer",
    "Designer",
    "Product Manager",
    "Data Scientist",
    "Developer Advocate",
    "QA Analyst",
    "SRE",
];
const DOMAIN: &[&str] = &["com", "org", "net", "io", "dev"];
const CURRENCY: &[&str] = &["USD", "EUR", "GBP", "JPY", "INR", "CAD", "AUD"];

/// Random u64 from the OS-seeded UUID generator (avoids another dependency).
fn rnd() -> u64 {
    let b = uuid::Uuid::new_v4().into_bytes();
    u64::from_le_bytes(b[..8].try_into().unwrap())
}

fn below(n: u64) -> u64 {
    rnd() % n.max(1)
}

fn pick<'a>(list: &[&'a str]) -> &'a str {
    list[below(list.len() as u64) as usize]
}

fn words(list: &[&str], n: usize) -> Vec<String> {
    (0..n).map(|_| pick(list).to_string()).collect()
}

fn sentence() -> String {
    let mut s = words(LOREM, 6 + below(6) as usize).join(" ");
    s[..1].make_ascii_uppercase();
    s + "."
}

fn alnum(n: usize) -> String {
    const C: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    (0..n)
        .map(|_| C[below(C.len() as u64) as usize] as char)
        .collect()
}

/// All supported names (for errors and autocompletion).
pub const NAMES: &[&str] = &[
    "guid",
    "randomUUID",
    "timestamp",
    "isoTimestamp",
    "randomInt",
    "randomBoolean",
    "randomFirstName",
    "randomLastName",
    "randomFullName",
    "randomUserName",
    "randomEmail",
    "randomExampleEmail",
    "randomPassword",
    "randomPhoneNumber",
    "randomCity",
    "randomCountry",
    "randomCountryCode",
    "randomStreetAddress",
    "randomColor",
    "randomHexColor",
    "randomIP",
    "randomIPV6",
    "randomUrl",
    "randomDomainName",
    "randomWord",
    "randomWords",
    "randomLoremWord",
    "randomLoremWords",
    "randomLoremSentence",
    "randomLoremParagraph",
    "randomAlphaNumeric",
    "randomCompanyName",
    "randomJobTitle",
    "randomPrice",
    "randomCurrencyCode",
    "randomDatePast",
    "randomDateFuture",
    "randomDateRecent",
];

pub fn generate(name: &str) -> Option<String> {
    let now = chrono::Utc::now();
    let days = |d: i64| {
        (now + chrono::Duration::days(d)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    };
    Some(match name {
        "guid" | "randomUUID" => uuid::Uuid::new_v4().to_string(),
        "timestamp" => now.timestamp().to_string(),
        "isoTimestamp" => now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "randomInt" => below(1001).to_string(),
        "randomBoolean" => (below(2) == 1).to_string(),
        "randomFirstName" => pick(FIRST).into(),
        "randomLastName" => pick(LAST).into(),
        "randomFullName" => format!("{} {}", pick(FIRST), pick(LAST)),
        "randomUserName" => format!(
            "{}.{}{}",
            pick(FIRST).to_lowercase(),
            pick(LAST).to_lowercase().replace('-', ""),
            below(100)
        ),
        "randomEmail" => format!(
            "{}.{}@example.{}",
            pick(FIRST).to_lowercase(),
            pick(LAST).to_lowercase().replace('-', ""),
            pick(DOMAIN)
        ),
        "randomExampleEmail" => format!("{}@example.com", pick(FIRST).to_lowercase()),
        "randomPassword" => alnum(15),
        "randomPhoneNumber" => format!(
            "{}-{:03}-{:04}",
            200 + below(800),
            below(1000),
            below(10000)
        ),
        "randomCity" => pick(CITY).into(),
        "randomCountry" => COUNTRY[below(COUNTRY.len() as u64) as usize].0.into(),
        "randomCountryCode" => COUNTRY[below(COUNTRY.len() as u64) as usize].1.into(),
        "randomStreetAddress" => format!("{} {}", 1 + below(9999), pick(STREET)),
        "randomColor" => pick(COLOR).into(),
        "randomHexColor" => format!("#{:06x}", below(0x1000000)),
        "randomIP" => format!(
            "{}.{}.{}.{}",
            1 + below(254),
            below(256),
            below(256),
            1 + below(254)
        ),
        "randomIPV6" => (0..8)
            .map(|_| format!("{:x}", below(0x10000)))
            .collect::<Vec<_>>()
            .join(":"),
        "randomDomainName" => format!("{}.{}", pick(WORD), pick(DOMAIN)),
        "randomUrl" => format!("https://{}.{}", pick(WORD), pick(DOMAIN)),
        "randomWord" => pick(WORD).into(),
        "randomWords" => words(WORD, 3).join(" "),
        "randomLoremWord" => pick(LOREM).into(),
        "randomLoremWords" => words(LOREM, 3).join(" "),
        "randomLoremSentence" => sentence(),
        "randomLoremParagraph" => (0..3).map(|_| sentence()).collect::<Vec<_>>().join(" "),
        "randomAlphaNumeric" => alnum(1),
        "randomCompanyName" => pick(COMPANY).into(),
        "randomJobTitle" => pick(JOB).into(),
        "randomPrice" => format!("{}.{:02}", below(1000), below(100)),
        "randomCurrencyCode" => pick(CURRENCY).into(),
        "randomDatePast" => days(-(1 + below(365) as i64)),
        "randomDateFuture" => days(1 + below(365) as i64),
        "randomDateRecent" => days(-(below(3) as i64)),
        _ => return None,
    })
}
