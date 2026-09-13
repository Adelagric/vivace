//! Port de `Composer\Json\JsonManipulator` (docs/reference/JsonManipulator.php)
//! pour le périmètre atteint par `require` et `remove` : édition textuelle
//! de composer.json par expressions régulières, pour ne toucher que la
//! partie modifiée et conserver la mise en forme du fichier.
//!
//! Les motifs sont ceux de Composer, compilés par pcre2 avec les mêmes
//! drapeaux (`s`, `x`, `i`, jamais `u`), sur les octets. Les valeurs
//! manipulées sont des `serde_json::Value` avec la convention de
//! `vivace_core::phpjson` : un objet réduit à `STDCLASS_MARKER` est un
//! `ArrayObject` (formaté `{…}` même vide), un objet vide est un tableau
//! PHP vide (formaté `[]`).
//!
//! Non porté : `repositories`, les listes (`addListItem`…), `addProperty`,
//! la branche `policy.*` de `removeConfigSetting`.
//!
//! Écarts connus avec PHP, tous sur des manifestes que `json_decode`
//! n'accepte pas ou n'accepte qu'à moitié : le seuil de
//! `pcre.backtrack_limit` (pcre2 n'expose pas `match_limit` ; l'erreur de
//! limite suit bien le chemin du `catch` de Composer, le seuil diffère) ;
//! un flottant hors plage (`1e999`, `INF` côté PHP, refusé ici) ; un
//! substitut UTF-16 isolé ou plus de 512 niveaux d'imbrication (PHP
//! continue avec `null` et ajoute une clé racine en double, ici une
//! erreur).

use pcre2::bytes::{Captures, Regex, RegexBuilder};
use serde::Deserialize as _;
use serde_json::{Map, Value};
use vivace_core::phpjson::{php_json_encode_with, STDCLASS_MARKER};

use crate::platform::is_platform_package;
use crate::version::preg_quote;

/// `PCRE2_ERROR_MATCHLIMIT` (pcre2.h), l'équivalent de
/// `PREG_BACKTRACK_LIMIT_ERROR` côté PHP.
const PCRE2_ERROR_MATCHLIMIT: i32 = -47;

/// `JsonFile::INDENT_DEFAULT`.
const INDENT_DEFAULT: &str = "    ";

/// `JsonManipulator::DEFINES` : la grammaire JSON en sous-motifs nommés.
const DEFINES: &str = r#"(?(DEFINE)
       (?<number>    -? (?= [1-9]|0(?!\d) ) \d++ (?:\.\d++)? (?:[eE] [+-]?+ \d++)? )
       (?<boolean>   true | false | null )
       (?<string>    " (?:[^"\\]*+ | \\ ["\\bfnrt\/] | \\ u [0-9A-Fa-f]{4} )* " )
       (?<array>     \[  (?:  (?&json) \s*+ (?: , (?&json) \s*+ )*+  )?+  \s*+ \] )
       (?<pair>      \s*+ (?&string) \s*+ : (?&json) \s*+ )
       (?<object>    \{  (?:  (?&pair)  (?: , (?&pair)  )*+  )?+  \s*+ \} )
       (?<json>      \s*+ (?: (?&number) | (?&boolean) | (?&string) | (?&array) | (?&object) ) )
    )"#;

#[derive(Debug, thiserror::Error)]
pub enum ManipulatorError {
    /// `The json file must be an object ({})`.
    #[error("The json file must be an object ({{}})")]
    NotAnObject,
    /// `JsonFile::parseJson` a échoué (ParsingException côté Composer).
    #[error("The input does not contain valid JSON\n{0}")]
    Parse(String),
    /// Une expression régulière a échoué là où Composer laisserait
    /// l'exception remonter.
    #[error("regex: {0}")]
    Regex(String),
    /// Chemin que Composer termine par une exception (`LogicException`,
    /// `InvalidArgumentException`, `TypeError`).
    #[error("{0}")]
    Logic(String),
    /// Méthode ou branche hors du périmètre porté.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

type Result<T> = std::result::Result<T, ManipulatorError>;

/// Drapeaux PCRE d'un motif (les lettres après le délimiteur en PHP).
#[derive(Clone, Copy, Default)]
struct Flags {
    s: bool,
    x: bool,
    i: bool,
    m: bool,
}

const SX: Flags = Flags {
    s: true,
    x: true,
    i: false,
    m: false,
};
const X: Flags = Flags {
    s: false,
    x: true,
    i: false,
    m: false,
};
const IX: Flags = Flags {
    s: false,
    x: true,
    i: true,
    m: false,
};
const I: Flags = Flags {
    s: false,
    x: false,
    i: true,
    m: false,
};
const S: Flags = Flags {
    s: true,
    x: false,
    i: false,
    m: false,
};
const NONE: Flags = Flags {
    s: false,
    x: false,
    i: false,
    m: false,
};

fn compile(pattern: &str, flags: Flags) -> Result<Regex> {
    RegexBuilder::new()
        .dotall(flags.s)
        .extended(flags.x)
        .caseless(flags.i)
        .multi_line(flags.m)
        .build(pattern)
        .map_err(|e| ManipulatorError::Regex(format!("{e} in `{pattern}`")))
}

/// `Preg::isMatch` : les captures, ou `None` si rien ne correspond.
fn captures<'s>(re: &Regex, subject: &'s str) -> Result<Option<Captures<'s>>> {
    re.captures(subject.as_bytes())
        .map_err(|e| ManipulatorError::Regex(e.to_string()))
}

/// `Preg::isMatch` dans un `catch` de `PREG_BACKTRACK_LIMIT_ERROR` : la
/// limite de correspondance de pcre2 (`PCRE2_ERROR_MATCHLIMIT`) suit ce
/// chemin (`false`), toute autre erreur remonte.
fn captures_or_limit<'s>(re: &Regex, subject: &'s str) -> Result<Option<Captures<'s>>> {
    match re.captures(subject.as_bytes()) {
        Ok(c) => Ok(c),
        Err(e) if e.code() == PCRE2_ERROR_MATCHLIMIT => Ok(None),
        Err(e) => Err(ManipulatorError::Regex(e.to_string())),
    }
}

/// Groupe nommé : `None` s'il n'a pas participé (`PREG_UNMATCHED_AS_NULL`).
fn named<'s>(caps: &Captures<'s>, name: &str) -> Option<&'s str> {
    caps.name(name)
        .and_then(|m| std::str::from_utf8(m.as_bytes()).ok())
}

fn group<'s>(caps: &Captures<'s>, i: usize) -> Option<&'s str> {
    caps.get(i)
        .and_then(|m| std::str::from_utf8(m.as_bytes()).ok())
}

/// `Preg::replaceCallback` sur toutes les occurrences.
fn replace_all(
    re: &Regex,
    subject: &str,
    mut f: impl FnMut(&Captures<'_>) -> Result<String>,
) -> Result<(String, usize)> {
    let mut out = String::with_capacity(subject.len());
    let mut last = 0;
    let mut count = 0;
    for caps in re.captures_iter(subject.as_bytes()) {
        let caps = caps.map_err(|e| ManipulatorError::Regex(e.to_string()))?;
        let m = caps
            .get(0)
            .ok_or_else(|| ManipulatorError::Regex("no group 0".into()))?;
        out.push_str(&subject[last..m.start()]);
        out.push_str(&f(&caps)?);
        last = m.end();
        count += 1;
    }
    out.push_str(&subject[last..]);
    Ok((out, count))
}

/// `Preg::replace` avec une chaîne de remplacement littérale (Composer
/// passe ses remplacements par `addcslashes(…, '\\$')`, ce qui revient à
/// les insérer tels quels).
fn replace_literal(re: &Regex, subject: &str, replacement: &str) -> Result<(String, usize)> {
    replace_all(re, subject, |_| Ok(replacement.to_owned()))
}

/// `JsonFile::encode` d'un scalaire ou d'une clé (`JSON_UNESCAPED_SLASHES |
/// JSON_UNESCAPED_UNICODE` ; `JSON_PRETTY_PRINT` est sans effet ici).
fn encode(value: &Value) -> Result<String> {
    php_json_encode_with(value, vivace_core::phpjson::FLAGS_JSONFILE)
        .map_err(|e| ManipulatorError::Logic(format!("JSON encoding failed: {e}")))
}

fn encode_str(s: &str) -> Result<String> {
    encode(&Value::String(s.to_owned()))
}

/// Profondeur maximale de `json_decode` (son paramètre `$depth` par défaut).
const PHP_JSON_DEPTH: usize = 512;

/// `json_decode` : serde avec deux écarts corrigés — la limite de
/// récursion (128 chez serde, 512 chez PHP, d'où une passe préalable qui
/// mesure l'imbrication hors chaînes) et le littéral `-0`, entier `0` pour
/// PHP mais flottant `-0.0` pour serde (réécrit avant l'analyse).
fn json_decode(s: &str) -> std::result::Result<Value, String> {
    let (normalized, depth) = scan_json(s);
    if depth > PHP_JSON_DEPTH {
        return Err("Maximum stack depth exceeded".into());
    }
    let mut de = serde_json::Deserializer::from_str(&normalized);
    de.disable_recursion_limit();
    let v = Value::deserialize(&mut de).map_err(|e| e.to_string())?;
    de.end().map_err(|e| e.to_string())?;
    Ok(v)
}

/// Une passe sur le texte JSON, hors chaînes : `-0` nu → `0`, et la
/// profondeur maximale d'imbrication.
fn scan_json(s: &str) -> (String, usize) {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    let mut max_depth = 0usize;
    let mut i = 0;
    let mut in_string = false;
    let mut last = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            match c {
                b'\\' => i += 1,
                b'"' => in_string = false,
                _ => {}
            }
        } else {
            match c {
                b'"' => in_string = true,
                b'[' | b'{' => {
                    depth += 1;
                    max_depth = max_depth.max(depth);
                }
                b']' | b'}' => depth = depth.saturating_sub(1),
                b'-' if bytes.get(i + 1) == Some(&b'0')
                    && !bytes
                        .get(i + 2)
                        .is_some_and(|n| matches!(n, b'.' | b'e' | b'E' | b'0'..=b'9')) =>
                {
                    out.push_str(&s[last..i]);
                    last = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    out.push_str(&s[last..]);
    (out, max_depth)
}

/// `JsonFile::parseJson` (décodage associatif).
fn parse_json(contents: &str) -> Result<Value> {
    json_decode(contents).map_err(ManipulatorError::Parse)
}

/// `@json_decode($s)` puis test de vérité PHP : `false` si le JSON est
/// invalide ou si la valeur décodée est fausse (`null`, `false`, `0`,
/// `""`, `"0"`, tableau vide — un objet vide est un tableau vide en mode
/// associatif, mais un `stdClass` vrai sinon). En mode objet, une clé
/// commençant par un octet nul est un échec de décodage
/// (`JSON_ERROR_INVALID_PROPERTY_NAME`).
fn decodes_truthy(s: &str, assoc: bool) -> bool {
    match json_decode(s) {
        Ok(v) => {
            if !assoc && has_nul_key(&v) {
                return false;
            }
            match &v {
                Value::Object(m) => !assoc || !m.is_empty(),
                other => php_truthy(other),
            }
        }
        Err(_) => false,
    }
}

fn has_nul_key(v: &Value) -> bool {
    match v {
        Value::Object(m) => m.iter().any(|(k, v)| k.starts_with('\0') || has_nul_key(v)),
        Value::Array(a) => a.iter().any(has_nul_key),
        _ => false,
    }
}

fn php_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !(s.is_empty() || s == "0"),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

/// `isset($decoded[$key])` : présent et non `null`.
fn isset<'v>(decoded: &'v Value, key: &str) -> Option<&'v Value> {
    php_index(decoded, key).filter(|v| !v.is_null())
}

/// `$value[$key]` sur un tableau PHP décodé : objet par clé, liste par
/// indice canonique.
fn php_index<'v>(value: &'v Value, key: &str) -> Option<&'v Value> {
    match value {
        Value::Object(m) => m.get(key),
        Value::Array(a) => canonical_index(key).and_then(|i| a.get(i)),
        // `isset("abc"[1])` : un décalage entier dans la chaîne (négatif
        // depuis la fin) est défini ; la valeur elle-même tient lieu de
        // caractère, ce qui suffit aux tests d'existence.
        Value::String(s) => php_int_key(key)
            .filter(|&i| {
                (0..s.len() as i64).contains(&(if i < 0 { i + s.len() as i64 } else { i }))
            })
            .map(|_| value),
        _ => None,
    }
}

/// Une clé que PHP convertit en entier, négatifs compris (`"-3"`, jamais
/// `"-0"` ni `"03"`).
fn php_int_key(key: &str) -> Option<i64> {
    if let Some(i) = canonical_index(key) {
        return i64::try_from(i).ok();
    }
    let rest = key.strip_prefix('-')?;
    if rest.starts_with(['1', '2', '3', '4', '5', '6', '7', '8', '9'])
        && rest.bytes().all(|b| b.is_ascii_digit())
    {
        return key.parse().ok();
    }
    None
}

/// Une clé que PHP convertit en entier (`"0"`, `"12"`, jamais `"012"`).
fn canonical_index(key: &str) -> Option<usize> {
    if key == "0" {
        return Some(0);
    }
    if key.starts_with(['1', '2', '3', '4', '5', '6', '7', '8', '9'])
        && key.bytes().all(|b| b.is_ascii_digit())
    {
        return key.parse().ok();
    }
    None
}

/// `array_is_list` d'un tableau décodé depuis un objet JSON : clés
/// `"0"`, `"1"`, … dans l'ordre.
fn php_is_list(m: &Map<String, Value>) -> bool {
    m.keys()
        .enumerate()
        .all(|(i, k)| canonical_index(k) == Some(i))
}

/// `$subName` est vrai au sens PHP (`if ($subName && …)`).
fn truthy_str(s: &str) -> bool {
    !(s.is_empty() || s == "0")
}

/// `strnatcmp` (ext/standard/strnatcmp.c, sensible à la casse).
pub fn strnatcmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering::{Equal, Greater, Less};
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.is_empty() || b.is_empty() {
        return a.len().cmp(&b.len());
    }
    // Une chaîne PHP est terminée par un octet nul : lire au-delà rend 0.
    let at = |s: &[u8], i: usize| s.get(i).copied().unwrap_or(0);
    let (mut ap, mut bp) = (0usize, 0usize);
    let (mut ca, mut cb) = (a[0], b[0]);
    while ca == b'0' && ap + 1 < a.len() && a[ap + 1].is_ascii_digit() {
        ap += 1;
        ca = a[ap];
    }
    while cb == b'0' && bp + 1 < b.len() && b[bp + 1].is_ascii_digit() {
        bp += 1;
        cb = b[bp];
    }
    let is_space = |c: u8| matches!(c, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r');
    loop {
        while is_space(ca) {
            ap += 1;
            ca = at(a, ap);
        }
        while is_space(cb) {
            bp += 1;
            cb = at(b, bp);
        }
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let fractional = ca == b'0' || cb == b'0';
            let result = if fractional {
                compare_left(a, &mut ap, b, &mut bp)
            } else {
                compare_right(a, &mut ap, b, &mut bp)
            };
            if result != Equal {
                return result;
            }
            if ap >= a.len() && bp >= b.len() {
                return Equal;
            }
            if ap >= a.len() {
                return Less;
            }
            if bp >= b.len() {
                return Greater;
            }
            ca = a[ap];
            cb = b[bp];
        }
        match ca.cmp(&cb) {
            Less => return Less,
            Greater => return Greater,
            Equal => {}
        }
        ap += 1;
        bp += 1;
        if ap >= a.len() && bp >= b.len() {
            return Equal;
        }
        if ap >= a.len() {
            return Less;
        }
        if bp >= b.len() {
            return Greater;
        }
        ca = a[ap];
        cb = b[bp];
    }
}

/// Deux nombres alignés à gauche (fractions) : la première différence gagne.
fn compare_left(a: &[u8], ap: &mut usize, b: &[u8], bp: &mut usize) -> std::cmp::Ordering {
    use std::cmp::Ordering::{Equal, Greater, Less};
    loop {
        let da = a.get(*ap).filter(|c| c.is_ascii_digit());
        let db = b.get(*bp).filter(|c| c.is_ascii_digit());
        match (da, db) {
            (None, None) => return Equal,
            (None, Some(_)) => return Less,
            (Some(_), None) => return Greater,
            (Some(x), Some(y)) => match x.cmp(y) {
                Less => return Less,
                Greater => return Greater,
                Equal => {}
            },
        }
        *ap += 1;
        *bp += 1;
    }
}

/// Deux nombres alignés à droite : le plus long gagne, sinon la première
/// différence (mémorisée dans `bias`).
fn compare_right(a: &[u8], ap: &mut usize, b: &[u8], bp: &mut usize) -> std::cmp::Ordering {
    use std::cmp::Ordering::{Equal, Greater, Less};
    let mut bias = Equal;
    loop {
        let da = a.get(*ap).filter(|c| c.is_ascii_digit());
        let db = b.get(*bp).filter(|c| c.is_ascii_digit());
        match (da, db) {
            (None, None) => return bias,
            (None, Some(_)) => return Less,
            (Some(_), None) => return Greater,
            (Some(x), Some(y)) => {
                if bias == Equal {
                    bias = x.cmp(y);
                }
            }
        }
        *ap += 1;
        *bp += 1;
    }
}

/// Le préfixe de tri de `sortPackages` : plateformes d'abord (`php`,
/// `hhvm`, `ext-*`, `lib-*`, les autres), puis les paquets.
fn sort_prefix(requirement: &str) -> String {
    if !is_platform_package(requirement) {
        return format!("5-{requirement}");
    }
    // Les cinq remplacements s'enchaînent sur la même chaîne ; une fois
    // préfixée d'un chiffre, elle n'est plus touchée par `^\D`.
    let mut s = requirement.to_owned();
    for (prefix, digit) in [("php", "0"), ("hhvm", "1"), ("ext", "2"), ("lib", "3")] {
        if s.starts_with(prefix) {
            s = format!("{digit}-{s}");
        }
    }
    if s.starts_with(|c: char| !c.is_ascii_digit()) {
        s = format!("4-{s}");
    }
    s
}

/// `sortPackages` : `uksort` stable par `strnatcmp` des préfixes. Dès que
/// le comparateur est appelé (deux entrées ou plus), une clé entière fait
/// échouer `isPlatformPackage(string $name)` (`strict_types`).
fn sort_packages(packages: &mut Map<String, Value>) -> Result<()> {
    if packages.len() >= 2 && packages.keys().any(|k| php_int_key(k).is_some()) {
        return Err(ManipulatorError::Logic(
            "PlatformRepository::isPlatformPackage(): Argument #1 ($name) must be of type string, int given".into(),
        ));
    }
    let mut entries: Vec<(String, Value)> = std::mem::take(packages).into_iter().collect();
    entries.sort_by(|(a, _), (b, _)| strnatcmp(&sort_prefix(a), &sort_prefix(b)));
    packages.extend(entries);
    Ok(())
}

/// Un objet réduit à la sentinelle `stdClass` (un `ArrayObject`).
fn is_stdclass_marker(m: &Map<String, Value>) -> bool {
    m.len() == 1 && m.contains_key(STDCLASS_MARKER)
}

pub struct JsonManipulator {
    contents: String,
    newline: &'static str,
    indent: String,
}

impl JsonManipulator {
    /// Constructeur : `trim`, `{}` → `{\n}`, détection du saut de ligne et
    /// de l'indentation.
    pub fn new(contents: &str) -> Result<Self> {
        let contents = contents.trim_matches([' ', '\t', '\n', '\r', '\0', '\x0B']);
        let contents = if contents.is_empty() { "{}" } else { contents };
        if captures(&compile(r"^\{(.*)\}$", S)?, contents)?.is_none() {
            return Err(ManipulatorError::NotAnObject);
        }
        let newline = if contents.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let contents = if contents == "{}" {
            format!("{{{newline}}}")
        } else {
            contents.to_owned()
        };
        let indent = detect_indenting(&contents)?;
        Ok(Self {
            contents,
            newline,
            indent,
        })
    }

    /// `getContents` : le texte suivi d'un saut de ligne.
    pub fn contents(&self) -> String {
        format!("{}{}", self.contents, self.newline)
    }

    /// `addLink` : ajoute ou remplace `$package: $constraint` dans la
    /// section `$type`, en conservant l'orthographe existante du nom.
    pub fn add_link(
        &mut self,
        link_type: &str,
        package: &str,
        constraint: &str,
        sort: bool,
    ) -> Result<bool> {
        let decoded = parse_json(&self.contents)?;
        if isset(&decoded, link_type).is_none() {
            let mut m = Map::new();
            m.insert(package.to_owned(), Value::String(constraint.to_owned()));
            return self.add_main_key(link_type, &Value::Object(m));
        }

        let regex = compile(
            &format!(
                r#"{DEFINES}^(?P<start>\s*\{{\s*(?:(?&string)\s*:\s*(?&json)\s*,\s*)*?)(?P<property>{}\s*:\s*)(?P<value>(?&json))(?P<end>.*)"#,
                preg_quote(&encode_str(link_type)?)
            ),
            SX,
        )?;
        let Some(caps) = captures(&regex, &self.contents)? else {
            return Ok(false);
        };
        let start = named(&caps, "start").unwrap_or("").to_owned();
        let property = named(&caps, "property").unwrap_or("").to_owned();
        let end = named(&caps, "end").unwrap_or("").to_owned();
        let mut links = named(&caps, "value").unwrap_or("").to_owned();

        // Le nom peut être écrit `vendor\/name` dans le fichier.
        let package_regex = preg_quote(package).replace('/', "\\\\?/");
        let regex = compile(
            &format!(r#"{DEFINES}"(?P<package>{package_regex})"(\s*:\s*)(?&string)"#),
            IX,
        )?;
        if let Some(pm) = captures(&regex, &links)? {
            let existing = named(&pm, "package").unwrap_or("").to_owned();
            let package_regex = preg_quote(&existing).replace('/', "\\\\?/");
            let regex = compile(
                &format!(r#"{DEFINES}"{package_regex}"(?P<separator>\s*:\s*)(?&string)"#),
                IX,
            )?;
            let name = encode_str(&existing.replace("\\/", "/"))?;
            links = replace_all(&regex, &links, |m| {
                Ok(format!(
                    "{name}{}\"{constraint}\"",
                    named(m, "separator").unwrap_or("")
                ))
            })?
            .0;
        } else {
            let regex = compile(r"^\s*\{\s*\S+.*?(\s*\}\s*)$", S)?;
            let tail = captures(&regex, &links)?.and_then(|m| group(&m, 1).map(str::to_owned));
            if let Some(tail) = tail {
                let regex = compile(&format!("{}$", preg_quote(&tail)), NONE)?;
                let replacement = format!(
                    ",{nl}{ind}{ind}{}: {}{tail}",
                    encode_str(package)?,
                    encode_str(constraint)?,
                    nl = self.newline,
                    ind = self.indent
                );
                links = replace_literal(&regex, &links, &replacement)?.0;
            } else {
                links = format!(
                    "{{{nl}{ind}{ind}{}: {}{nl}{ind}}}",
                    encode_str(package)?,
                    encode_str(constraint)?,
                    nl = self.newline,
                    ind = self.indent
                );
            }
        }

        if sort {
            let requirements = json_decode(&links).map_err(|e| {
                ManipulatorError::Logic(format!("sortPackages(): links are not JSON: {e}"))
            })?;
            links = match requirements {
                Value::Object(mut m) => {
                    sort_packages(&mut m)?;
                    self.format(&Value::Object(m), 0, false)?
                }
                // Une liste est un tableau PHP à clés entières : `uksort`
                // ne compare rien s'il y a au plus un élément.
                Value::Array(a) => {
                    let mut m = list_to_map(a);
                    sort_packages(&mut m)?;
                    self.format(&Value::Object(m), 0, false)?
                }
                _ => {
                    return Err(ManipulatorError::Logic(
                        "sortPackages(): Argument #1 ($packages) must be of type array".into(),
                    ))
                }
            };
        }

        self.contents = format!("{start}{property}{links}{end}");
        Ok(true)
    }

    /// `removeConfigSetting` (hors `policy.*`).
    pub fn remove_config_setting(&mut self, name: &str) -> Result<bool> {
        if name.starts_with("policy.") && name.matches('.').count() >= 2 {
            return Err(ManipulatorError::Unsupported(format!(
                "removeConfigSetting({name}): policy lists"
            )));
        }
        self.remove_sub_node("config", name)
    }

    /// `addSubNode` : ajoute ou remplace `$name` (ou `$name.$subName` pour
    /// `config`/`extra`/`scripts`) dans l'objet `$mainNode`.
    pub fn add_sub_node(
        &mut self,
        main_node: &str,
        name: &str,
        value: &Value,
        append: bool,
    ) -> Result<bool> {
        let decoded = parse_json(&self.contents)?;
        let (name, sub_name) = split_sub_name(main_node, name);

        if isset(&decoded, main_node).is_none() {
            let inner = match sub_name {
                Some(sub) => {
                    let mut m = Map::new();
                    m.insert(sub.to_owned(), value.clone());
                    Value::Object(m)
                }
                None => value.clone(),
            };
            let mut m = Map::new();
            m.insert(name.to_owned(), inner);
            self.add_main_key(main_node, &Value::Object(m))?;
            return Ok(true);
        }

        let node_regex = self.node_regex(main_node)?;
        let Some(caps) = captures_or_limit(&node_regex, &self.contents)? else {
            return Ok(false);
        };
        let mut children = named(&caps, "content").unwrap_or("").to_owned();

        if !decodes_truthy(&children, false) {
            return Ok(false);
        }

        let child_regex = compile(
            &format!(
                r#"{DEFINES}(?P<start>"{}"\s*:\s*)(?P<content>(?&json))(?P<end>,?)"#,
                preg_quote(name)
            ),
            X,
        )?;
        if captures(&child_regex, &children)?.is_some() {
            children = replace_all(&child_regex, &children, |m| {
                let content = named(m, "content").unwrap_or("");
                let value = match sub_name {
                    Some(sub) => {
                        let mut cur = match json_decode(content) {
                            Ok(Value::Object(m)) => m,
                            Ok(Value::Array(a)) => list_to_map(a),
                            _ => Map::new(),
                        };
                        cur.insert(sub.to_owned(), value.clone());
                        Value::Object(cur)
                    }
                    None => value.clone(),
                };
                Ok(format!(
                    "{}{}{}",
                    named(m, "start").unwrap_or(""),
                    self.format(&value, 1, false)?,
                    named(m, "end").unwrap_or("")
                ))
            })?
            .0;
        } else {
            let regex = compile(
                r"^\{(?P<leadingspace>\s*?)(?P<content>\S+.*?)?(?P<trailingspace>\s*)\}$",
                S,
            )?;
            let Some(m) = captures(&regex, &children)? else {
                return Err(ManipulatorError::Logic(format!(
                    "Nothing matched above for: {children}"
                )));
            };
            let leading = named(&m, "leadingspace").unwrap_or("").to_owned();
            let trailing = named(&m, "trailingspace").unwrap_or("").to_owned();
            let has_content = named(&m, "content").is_some();
            let value = match sub_name {
                Some(sub) => {
                    let mut m = Map::new();
                    m.insert(sub.to_owned(), value.clone());
                    Value::Object(m)
                }
                None => value.clone(),
            };
            let entry = format!("{}: {}", encode_str(name)?, self.format(&value, 1, false)?);
            if has_content {
                if append {
                    let regex = compile(&format!("{trailing}}}$"), NONE)?;
                    let replacement = format!(
                        ",{nl}{ind}{ind}{entry}{trailing}}}",
                        nl = self.newline,
                        ind = self.indent
                    );
                    children = replace_literal(&regex, &children, &replacement)?.0;
                } else {
                    let regex = compile(&format!("^{{{leading}"), NONE)?;
                    let replacement = format!(
                        "{{{leading}{entry},{nl}{ind}{ind}",
                        nl = self.newline,
                        ind = self.indent
                    );
                    children = replace_literal(&regex, &children, &replacement)?.0;
                }
            } else {
                children = format!(
                    "{{{nl}{ind}{ind}{entry}{trailing}}}",
                    nl = self.newline,
                    ind = self.indent
                );
            }
        }

        self.contents = replace_all(&node_regex, &self.contents, |m| {
            Ok(format!(
                "{}{children}{}",
                named(m, "start").unwrap_or(""),
                named(m, "end").unwrap_or("")
            ))
        })?
        .0;
        Ok(true)
    }

    /// `removeSubNode` : retire `$name` (ou `$name.$subName`) de l'objet
    /// `$mainNode`.
    pub fn remove_sub_node(&mut self, main_node: &str, name: &str) -> Result<bool> {
        let decoded = parse_json(&self.contents)?;
        if !php_index(&decoded, main_node).is_some_and(php_truthy) {
            return Ok(true);
        }

        let node_regex = self.node_regex(main_node)?;
        let Some(caps) = captures_or_limit(&node_regex, &self.contents)? else {
            return Ok(false);
        };
        let children = named(&caps, "content").unwrap_or("").to_owned();

        if !decodes_truthy(&children, true) {
            return Ok(false);
        }

        let (name, sub_name) = split_sub_name(main_node, name);

        let node = php_index(&decoded, main_node).unwrap_or(&Value::Null);
        let Some(entry) = isset(node, name) else {
            return Ok(true);
        };
        if let Some(sub) = sub_name {
            if truthy_str(sub) && isset(entry, sub).is_none() {
                return Ok(true);
            }
        }

        let key_regex = preg_quote(name).replace('/', "\\\\?/");
        let children_clean = if captures(&compile(&format!(r#""{key_regex}"\s*:"#), I)?, &children)?
            .is_some()
        {
            let all = compile(&format!(r#"{DEFINES}"{key_regex}"\s*:\s*(?:(?&json))"#), X)?;
            let mut best = String::new();
            let mut any = false;
            for m in all.captures_iter(children.as_bytes()) {
                let m = m.map_err(|e| ManipulatorError::Regex(e.to_string()))?;
                any = true;
                let whole = group(&m, 0).unwrap_or("");
                if best.len() < whole.len() {
                    best = whole.to_owned();
                }
            }
            if !any {
                return Err(ManipulatorError::Logic(
                    "JsonManipulator: $childrenClean is not defined. Please report at https://github.com/composer/composer/issues/new.".into(),
                ));
            }
            let (mut clean, count) = replace_literal(
                &compile(&format!(r",\s*{}", preg_quote(&best)), I)?,
                &children,
                "",
            )?;
            if count != 1 {
                let (clean2, count2) = replace_literal(
                    &compile(&format!(r"{}\s*,?\s*", preg_quote(&best)), I)?,
                    &clean,
                    "",
                )?;
                if count2 != 1 {
                    return Ok(false);
                }
                clean = clean2;
            }
            clean
        } else {
            children.clone()
        };

        let regex = compile(r"^\{\s*?(?P<content>\S+.*?)?(?P<trailingspace>\s*)\}$", S)?;
        if let Some(m) = captures(&regex, &children_clean)? {
            if named(&m, "content").is_none() {
                let empty = format!("{{{}{}}}", self.newline, self.indent);
                self.contents = replace_all(&node_regex, &self.contents, |m| {
                    Ok(format!(
                        "{}{empty}{}",
                        named(m, "start").unwrap_or(""),
                        named(m, "end").unwrap_or("")
                    ))
                })?
                .0;
                if let Some(sub) = sub_name {
                    let cur = json_decode(&children).map_err(ManipulatorError::Parse)?;
                    let inner = unset_sub(&cur, name, sub)?;
                    self.add_sub_node(main_node, name, &inner, true)?;
                }
                return Ok(true);
            }
        }

        let replacement = match sub_name {
            Some(sub) => {
                let content = named(&caps, "content").unwrap_or("");
                let mut cur = match json_decode(content) {
                    Ok(Value::Object(m)) => m,
                    Ok(Value::Array(a)) => list_to_map(a),
                    _ => Map::new(),
                };
                let inner = unset_sub(&Value::Object(cur.clone()), name, sub)?;
                cur.insert(name.to_owned(), inner);
                self.format(&Value::Object(cur), 0, true)?
            }
            None => children_clean,
        };
        self.contents = replace_all(&node_regex, &self.contents, |m| {
            Ok(format!(
                "{}{replacement}{}",
                named(m, "start").unwrap_or(""),
                named(m, "end").unwrap_or("")
            ))
        })?
        .0;
        Ok(true)
    }

    /// `addMainKey` : remplace la clé racine si elle existe, sinon
    /// l'ajoute en fin d'objet.
    pub fn add_main_key(&mut self, key: &str, content: &Value) -> Result<bool> {
        let decoded = parse_json(&self.contents)?;
        let content = self.format(content, 0, false)?;
        let encoded_key = encode_str(key)?;

        let regex = compile(
            &format!(
                r#"{DEFINES}^(?P<start>\s*\{{\s*(?:(?&string)\s*:\s*(?&json)\s*,\s*)*?)(?P<key>{}\s*:\s*(?&json))(?P<end>.*)"#,
                preg_quote(&encoded_key)
            ),
            SX,
        )?;
        if isset(&decoded, key).is_some() {
            if let Some(m) = captures(&regex, &self.contents)? {
                let key_text = named(&m, "key").unwrap_or("");
                if !decodes_truthy(&format!("{{{key_text}}}"), false) {
                    return Ok(false);
                }
                self.contents = format!(
                    "{}{encoded_key}: {content}{}",
                    named(&m, "start").unwrap_or(""),
                    named(&m, "end").unwrap_or("")
                );
                return Ok(true);
            }
        }

        let regex = compile(r"[^{\s](\s*)\}$", NONE)?;
        if let Some(m) = captures(&regex, &self.contents)? {
            let ws = group(&m, 1).unwrap_or("").to_owned();
            let regex = compile(&format!(r"{ws}\}}$"), NONE)?;
            let replacement = format!(
                ",{nl}{ind}{encoded_key}: {content}{nl}}}",
                nl = self.newline,
                ind = self.indent
            );
            self.contents = replace_literal(&regex, &self.contents, &replacement)?.0;
            return Ok(true);
        }

        let regex = compile(r"\}$", NONE)?;
        let replacement = format!(
            "{ind}{encoded_key}: {content}{nl}}}",
            nl = self.newline,
            ind = self.indent
        );
        self.contents = replace_literal(&regex, &self.contents, &replacement)?.0;
        Ok(true)
    }

    /// `removeMainKey`.
    pub fn remove_main_key(&mut self, key: &str) -> Result<bool> {
        let decoded = parse_json(&self.contents)?;
        if php_index(&decoded, key).is_none() {
            return Ok(true);
        }

        let regex = compile(
            &format!(
                r#"{DEFINES}^(?P<start>\s*\{{\s*(?:(?&string)\s*:\s*(?&json)\s*,\s*)*?)(?P<removal>{}\s*:\s*(?&json))\s*,?\s*(?P<end>.*)"#,
                preg_quote(&encode_str(key)?)
            ),
            SX,
        )?;
        let Some(m) = captures(&regex, &self.contents)? else {
            return Ok(false);
        };
        let mut start = named(&m, "start").unwrap_or("").to_owned();
        let removal = named(&m, "removal").unwrap_or("");
        let end = named(&m, "end").unwrap_or("").to_owned();

        if !decodes_truthy(&format!("{{{removal}}}"), false) {
            return Ok(false);
        }

        // Dernière clé retirée : la virgule qui la précédait part avec elle.
        if captures(&compile(r",\s*$", NONE)?, &start)?.is_some()
            && captures(&compile(r"^\}$", NONE)?, &end)?.is_some()
        {
            let regex = compile(r",(\s*)$", NONE)?;
            let stripped =
                replace_all(&regex, &start, |m| Ok(group(m, 1).unwrap_or("").to_owned()))?.0;
            let chars: Vec<char> = self.indent.chars().collect();
            start = stripped.trim_end_matches(chars.as_slice()).to_owned();
        }

        self.contents = format!("{start}{end}");
        if captures(&compile(r"^\{\s*\}\s*$", NONE)?, &self.contents)?.is_some() {
            self.contents = "{\n}".to_owned();
        }
        Ok(true)
    }

    /// `removeMainKeyIfEmpty` : retire la clé si c'est un tableau vide.
    pub fn remove_main_key_if_empty(&mut self, key: &str) -> Result<bool> {
        let decoded = parse_json(&self.contents)?;
        let Some(v) = php_index(&decoded, key) else {
            return Ok(true);
        };
        let empty = match v {
            Value::Array(a) => a.is_empty(),
            Value::Object(m) => m.is_empty(),
            _ => false,
        };
        if empty {
            return self.remove_main_key(key);
        }
        Ok(true)
    }

    /// `format` : mise en forme d'une valeur à la profondeur donnée, avec
    /// l'indentation et le saut de ligne du fichier.
    pub fn format(&self, data: &Value, depth: usize, was_object: bool) -> Result<String> {
        let indent = |n: usize| self.indent.repeat(n);
        let empty_object = |was_object: bool| {
            if was_object {
                format!("{{{}{}}}", self.newline, indent(depth + 1))
            } else {
                "[]".to_owned()
            }
        };
        match data {
            Value::Object(m) if is_stdclass_marker(m) => Ok(empty_object(true)),
            Value::Object(m) if m.is_empty() => Ok(empty_object(was_object)),
            Value::Array(a) if a.is_empty() => Ok(empty_object(was_object)),
            Value::Array(a) => {
                let items = a
                    .iter()
                    .map(|v| self.format(v, depth + 1, false))
                    .collect::<Result<Vec<_>>>()?;
                Ok(format!("[{}]", items.join(", ")))
            }
            Value::Object(m) if php_is_list(m) => {
                let items = m
                    .values()
                    .map(|v| self.format(v, depth + 1, false))
                    .collect::<Result<Vec<_>>>()?;
                Ok(format!("[{}]", items.join(", ")))
            }
            Value::Object(m) => {
                let mut elems = Vec::with_capacity(m.len());
                for (k, v) in m {
                    elems.push(format!(
                        "{}{}: {}",
                        indent(depth + 2),
                        encode_str(k)?,
                        self.format(v, depth + 1, false)?
                    ));
                }
                Ok(format!(
                    "{{{nl}{}{nl}{}}}",
                    elems.join(&format!(",{}", self.newline)),
                    indent(depth + 1),
                    nl = self.newline
                ))
            }
            scalar => encode(scalar),
        }
    }

    /// Le motif commun à `addSubNode`/`removeSubNode` : tout jusqu'à la
    /// clé racine, puis son objet, puis le reste.
    fn node_regex(&self, main_node: &str) -> Result<Regex> {
        compile(
            &format!(
                r#"{DEFINES}^(?P<start> \s* \{{ \s* (?: (?&string) \s* : (?&json) \s* , \s* )*?{}\s*:\s*)(?P<content>(?&object))(?P<end>.*)"#,
                preg_quote(&encode_str(main_node)?)
            ),
            SX,
        )
    }
}

/// `config`/`extra`/`scripts` acceptent `name.sub` pour viser une sous-clé.
fn split_sub_name<'a>(main_node: &str, name: &'a str) -> (&'a str, Option<&'a str>) {
    if matches!(main_node, "config" | "extra" | "scripts") {
        if let Some((n, s)) = name.split_once('.') {
            return (n, Some(s));
        }
    }
    (name, None)
}

/// Une liste décodée vue comme tableau PHP à clés entières.
fn list_to_map(a: Vec<Value>) -> Map<String, Value> {
    a.into_iter()
        .enumerate()
        .map(|(i, v)| (i.to_string(), v))
        .collect()
}

/// `unset($cur[$name][$sub]); if ($cur[$name] === []) new ArrayObject`.
fn unset_sub(cur: &Value, name: &str, sub: &str) -> Result<Value> {
    let inner = php_index(cur, name).cloned().unwrap_or(Value::Null);
    let mut m = match inner {
        Value::Object(m) => m,
        Value::Array(a) => list_to_map(a),
        // `unset` sur `null` ou `false` ne fait rien (et `=== []` est faux).
        Value::Null => return Ok(Value::Null),
        Value::Bool(false) => return Ok(Value::Bool(false)),
        other => {
            return Err(ManipulatorError::Logic(format!(
                "Cannot unset offset in a non-array variable ({other})"
            )))
        }
    };
    m.shift_remove(sub);
    if m.is_empty() {
        return Ok(vivace_core::phpjson::empty_stdclass());
    }
    Ok(Value::Object(m))
}

/// `JsonFile::detectIndenting` : la première ligne `^[ \t]+"`.
pub fn detect_indenting(json: &str) -> Result<String> {
    let re = compile(r#"^([ \t]+)""#, Flags { m: true, ..NONE })?;
    Ok(captures(&re, json)?
        .and_then(|c| group(&c, 1).map(str::to_owned))
        .unwrap_or_else(|| INDENT_DEFAULT.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::{Equal, Greater, Less};

    #[test]
    fn strnatcmp_matches_php() {
        assert_eq!(strnatcmp("0-php", "1-hhvm"), Less);
        assert_eq!(strnatcmp("ext-json", "ext-Json"), Greater);
        assert_eq!(strnatcmp("a10", "a9"), Greater);
        assert_eq!(strnatcmp("a 1", "a1"), Equal);
        assert_eq!(strnatcmp("a01", "a1"), Less);
        assert_eq!(strnatcmp("1.10", "1.9"), Greater);
        assert_eq!(strnatcmp("01", "1"), Equal);
        assert_eq!(strnatcmp("", "a"), Less);
        assert_eq!(strnatcmp("abc", "ab"), Greater);
    }

    #[test]
    fn sort_prefixes() {
        assert_eq!(sort_prefix("php"), "0-php");
        assert_eq!(sort_prefix("php-64bit"), "0-php-64bit");
        assert_eq!(sort_prefix("hhvm"), "1-hhvm");
        assert_eq!(sort_prefix("ext-json"), "2-ext-json");
        assert_eq!(sort_prefix("lib-icu"), "3-lib-icu");
        assert_eq!(sort_prefix("composer-plugin-api"), "4-composer-plugin-api");
        assert_eq!(sort_prefix("acme/lib"), "5-acme/lib");
    }

    #[test]
    fn add_link_to_existing_section() {
        let mut m =
            JsonManipulator::new("{\n    \"require\": {\n        \"a/b\": \"^1\"\n    }\n}\n")
                .expect("new");
        assert!(m.add_link("require", "c/d", "^2", false).expect("add"));
        assert_eq!(
            m.contents(),
            "{\n    \"require\": {\n        \"a/b\": \"^1\",\n        \"c/d\": \"^2\"\n    }\n}\n"
        );
        assert!(m.add_link("require", "A/B", "^3", true).expect("replace"));
        assert_eq!(
            m.contents(),
            "{\n    \"require\": {\n        \"a/b\": \"^3\",\n        \"c/d\": \"^2\"\n    }\n}\n"
        );
    }

    #[test]
    fn add_link_creates_section_and_remove_drops_it() {
        let mut m = JsonManipulator::new("{}").expect("new");
        assert!(m.add_link("require", "a/b", "^1", false).expect("add"));
        assert_eq!(
            m.contents(),
            "{\n    \"require\": {\n        \"a/b\": \"^1\"\n    }\n}\n"
        );
        assert!(m.remove_sub_node("require", "a/b").expect("remove"));
        assert_eq!(m.contents(), "{\n    \"require\": {\n    }\n}\n");
        assert!(m.remove_main_key_if_empty("require").expect("drop"));
        assert_eq!(m.contents(), "{\n}\n");
    }

    #[test]
    fn crlf_and_tabs_are_kept() {
        let mut m =
            JsonManipulator::new("{\r\n\t\"require\": {\r\n\t\t\"a/b\": \"^1\"\r\n\t}\r\n}")
                .expect("new");
        assert!(m.add_link("require", "c/d", "^2", false).expect("add"));
        assert_eq!(
            m.contents(),
            "{\r\n\t\"require\": {\r\n\t\t\"a/b\": \"^1\",\r\n\t\t\"c/d\": \"^2\"\r\n\t}\r\n}\r\n"
        );
    }

    #[test]
    fn config_sub_key_removal() {
        let mut m = JsonManipulator::new(
            "{\n    \"config\": {\n        \"allow-plugins\": {\n            \"a/b\": true,\n            \"c/d\": false\n        }\n    }\n}",
        )
        .expect("new");
        assert!(m
            .remove_config_setting("allow-plugins.a/b")
            .expect("remove"));
        assert_eq!(
            m.contents(),
            "{\n    \"config\": {\n        \"allow-plugins\": {\n            \"c/d\": false\n        }\n    }\n}\n"
        );
        assert!(m
            .remove_config_setting("allow-plugins.c/d")
            .expect("remove"));
        assert_eq!(
            m.contents(),
            "{\n    \"config\": {\n        \"allow-plugins\": {\n        }\n    }\n}\n"
        );
    }
}
