//! Dynamic results computed from the query: calculator, web search (`? text`), `kill <app>`,
//! clipboard history (`cb`) and emojis (`:name`, see `emoji.rs`).
use super::{Entry, Kind, OpenMods};
use std::sync::OnceLock;
use tauri::{AppHandle, Manager};

/// Server-side French strings (entries are built here, not in the front).
mod text {
    pub const WEB_TITLE: &str = "Rechercher « {q} » sur le web";
    pub const WEB_SUB: &str = "Entrée : ouvrir dans le navigateur";
    pub const CALC_SUB_HINT: &str = "Entrée : copier";
    pub const KILL_SUB: &str = "Entrée : quitter · Maj+Entrée : forcer";
}

pub struct RunningApp {
    pub id: String,
    pub name: String,
}

pub struct Ctx {
    pub web_url: String,
    pub running: Vec<RunningApp>,
    /// Clipboard history (newest first) and the current time, only filled for `cb` queries.
    pub clip: Vec<super::clipboard::Item>,
    pub now: u64,
    /// Shared monitor (name, other PC's label) when configured.
    pub display: Option<(String, String)>,
}

/// "écran", "source", "autre pc" or the other PC's name (accents ignored).
fn wants_display_switch(q: &str, label: &str) -> bool {
    let fold = |s: &str| s.trim().to_lowercase().replace(['é', 'è', 'ê'], "e");
    let q = fold(q);
    if q.len() < 3 {
        return false;
    }
    let label = fold(label);
    ["ecran", "source", "autre pc", "basculer"].iter().any(|k| k.starts_with(&q) || q.starts_with(k))
        || (!label.is_empty() && (label.starts_with(&q) || q.contains(&label)))
}

// ---------- calculator ----------

const MAX_DEPTH: u32 = 64;
const MAX_LEN: usize = 200;

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    ops: usize,
    depth: u32,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i] == b' ' {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        loop {
            if self.eat(b'+') {
                self.ops += 1;
                v += self.term()?;
            } else if self.eat(b'-') {
                self.ops += 1;
                v -= self.term()?;
            } else {
                return Some(v);
            }
        }
    }
    fn term(&mut self) -> Option<f64> {
        let mut v = self.unary()?;
        loop {
            if self.eat(b'*') {
                self.ops += 1;
                v *= self.unary()?;
            } else if self.eat(b'/') {
                self.ops += 1;
                let d = self.unary()?;
                if d == 0.0 {
                    return None;
                }
                v /= d;
            } else if self.eat(b'%') {
                self.ops += 1;
                let d = self.unary()?;
                if d == 0.0 {
                    return None;
                }
                v %= d;
            } else {
                return Some(v);
            }
        }
    }
    fn unary(&mut self) -> Option<f64> {
        self.depth += 1;
        let r = if self.depth > MAX_DEPTH { None } else { self.unary_inner() };
        self.depth -= 1;
        r
    }
    fn unary_inner(&mut self) -> Option<f64> {
        if self.eat(b'-') {
            return Some(-self.unary()?);
        }
        if self.eat(b'+') {
            return self.unary();
        }
        let base = self.atom()?;
        if self.eat(b'^') {
            self.ops += 1;
            return Some(base.powf(self.unary()?)); // right-associative
        }
        Some(base)
    }
    fn atom(&mut self) -> Option<f64> {
        self.ws();
        if self.eat(b'(') {
            let v = self.expr()?;
            return self.eat(b')').then_some(v);
        }
        let start = self.i;
        if self.s.get(self.i).is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
            while self.s.get(self.i).is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
                self.i += 1;
            }
            return std::str::from_utf8(&self.s[start..self.i]).ok()?.parse().ok();
        }
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_alphabetic()) {
            self.i += 1;
        }
        match &self.s[start..self.i] {
            b"pi" => Some(std::f64::consts::PI),
            b"sqrt" => {
                if !self.eat(b'(') {
                    return None;
                }
                let v = self.expr()?;
                if !self.eat(b')') || v < 0.0 {
                    return None;
                }
                self.ops += 1; // a function call counts as an operation
                Some(v.sqrt())
            }
            _ => None,
        }
    }
}

/// Evaluates `q` when it is a real expression (needs at least one operator or `sqrt`).
/// `%` is the modulo operator. `,` and `.` are both decimal separators.
pub fn calc(q: &str) -> Option<f64> {
    if q.len() > MAX_LEN {
        return None;
    }
    let norm = q.trim().to_lowercase().replace(',', ".");
    let mut p = Parser { s: norm.as_bytes(), i: 0, ops: 0, depth: 0 };
    let v = p.expr()?;
    p.ws();
    (p.i == p.s.len() && p.ops > 0 && v.is_finite()).then_some(v)
}

/// French formatting: comma decimal, no trailing zeros, at most 10 significant digits.
pub fn format_fr(v: f64) -> String {
    let rounded: f64 = format!("{:.9e}", v).parse().unwrap_or(v);
    if rounded == 0.0 {
        return "0".into();
    }
    format!("{rounded}").replace('.', ",")
}

// ---------- web ----------

fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn is_http(url: &str) -> bool {
    let l = url.to_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

// ---------- provider ----------

fn entry(id: String, title: String, subtitle: String, payload: String) -> Entry {
    Entry { id, kind: Kind::Command, title, subtitle, icon: None, keywords: vec![payload], valo: None }
}

/// The text after a case-insensitive `kill ` prefix. Byte-safe: never slices inside a char.
fn kill_needle(q: &str) -> Option<&str> {
    q.as_bytes().get(..5).is_some_and(|b| b.eq_ignore_ascii_case(b"kill ")).then(|| q.get(5..)).flatten()
}

fn is_own(windows: &[crate::dock_model::WindowRef]) -> bool {
    windows.iter().any(|w| crate::actions::is_own_window(w.hwnd))
}

pub fn dynamic(q: &str, ctx: &Ctx) -> Vec<Entry> {
    let mut out = Vec::new();
    if let Some(text) = q.strip_prefix("? ") {
        let text = text.trim();
        if !text.is_empty() {
            let url = ctx.web_url.replace("%s", &url_encode(text));
            out.push(entry("cmd:web".into(), text::WEB_TITLE.replace("{q}", text), text::WEB_SUB.into(), url));
        }
        return out;
    }
    if let Some(filter) = super::clipboard::cb_arg(q) {
        return super::clipboard::entries(filter, &ctx.clip, ctx.now);
    }
    if q.starts_with(':') {
        return super::emoji::lookup(q);
    }
    if let Some(v) = calc(q) {
        let r = format_fr(v);
        out.push(entry(
            "cmd:calc".into(),
            format!("= {r}"),
            format!("{} · {}", q.trim(), text::CALC_SUB_HINT),
            r,
        ));
        return out;
    }
    if let Some((monitor, label)) = &ctx.display {
        if wants_display_switch(q, label) {
            let other = if label.trim().is_empty() { "l'autre PC" } else { label.trim() };
            out.push(entry(
                "cmd:display".into(),
                format!("Basculer l'écran {monitor}"),
                format!("Ce PC ⇄ {other}"),
                String::new(),
            ));
        }
    }
    for a in crate::power_menu::match_query(q) {
        let sub = if a.is_destructive() { crate::power_menu::text::SUB_ASK } else { crate::power_menu::text::SUB_RUN };
        out.push(entry(format!("cmd:power:{}", a.id()), a.label().into(), sub.into(), a.id().into()));
    }
    if let Some(needle) = kill_needle(q) {
        let needle = needle.trim().to_lowercase();
        for a in &ctx.running {
            if a.name.eq_ignore_ascii_case("lucent") || !a.name.to_lowercase().contains(&needle) {
                continue;
            }
            out.push(entry(format!("cmd:kill:{}", a.id), a.name.clone(), text::KILL_SUB.into(), a.id.clone()));
        }
    }
    out
}

// ---------- wiring ----------

static APP: OnceLock<AppHandle> = OnceLock::new();

fn provider(q: &str) -> Vec<Entry> {
    if super::valo::is_valo_query(q) {
        return super::valo::provider();
    }
    let Some(app) = APP.get() else { return Vec::new() };
    let settings = app.state::<crate::settings::SettingsStore>().get();
    let web_url = settings.search.web_url;
    let ds = settings.display_switch;
    let display = (!ds.monitor.trim().is_empty()).then(|| (ds.monitor.clone(), ds.other_label.clone()));
    let running = if kill_needle(q).is_some() {
        crate::current_items(app)
            .into_iter()
            .filter(|i| !i.windows.is_empty() && !is_own(&i.windows))
            .map(|i| RunningApp { id: i.id, name: i.name })
            .collect()
    } else {
        Vec::new()
    };
    let clip = if super::clipboard::cb_arg(q).is_some() { super::clipboard::snapshot() } else { Vec::new() };
    dynamic(q, &Ctx { web_url, running, clip, now: super::clipboard::now_secs(), display })
}

fn open(e: &Entry, mods: OpenMods) {
    let payload = e.keywords.first().cloned().unwrap_or_default();
    if e.id == "cmd:calc" {
        super::clipboard::clipboard_write(&payload);
    } else if e.id == "cmd:web" {
        if !is_http(&payload) {
            eprintln!("lucent: web search url must be http(s), ignoring {payload:?}");
            return;
        }
        #[cfg(windows)]
        open_url(&payload);
    } else if e.id == "cmd:display" {
        #[cfg(windows)]
        if let Some(app) = APP.get() {
            let s = app.state::<crate::settings::SettingsStore>().get().display_switch;
            crate::system::display_input::win::switch(s, None);
        }
    } else if e.id.starts_with("cmd:power:") {
        // The search page asks for a second Enter on the destructive ones before calling.
        if let Some(action) = crate::power_menu::PowerAction::parse(&payload) {
            #[cfg(windows)]
            crate::power_menu::win::spawn(action, false);
            #[cfg(not(windows))]
            let _ = action;
        }
    } else if e.id.starts_with("cmd:kill:") {
        let Some(app) = APP.get() else { return };
        let Some(item) = crate::current_items(app).into_iter().find(|i| i.id == payload) else { return };
        if item.name.eq_ignore_ascii_case("lucent") || is_own(&item.windows) {
            return;
        }
        if mods.shift {
            crate::actions::end_task(&item.windows);
        } else {
            crate::actions::quit(&item.windows);
        }
    }
}

#[cfg(windows)]
fn open_url(url: &str) {
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let target = HSTRING::from(url);
    let r = unsafe { ShellExecuteW(None, w!("open"), &target, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
    if r.0 as isize <= 32 {
        eprintln!("lucent: cannot open web search url ({})", r.0 as isize);
    }
}

pub fn init(app: AppHandle, index: &super::SearchIndex) {
    let _ = APP.set(app);
    super::register_opener(Kind::Command, open);
    index.set_dynamic_provider(Some(provider));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(names: &[&str]) -> Ctx {
        Ctx {
            web_url: "https://www.google.com/search?q=%s".into(),
            running: names.iter().map(|n| RunningApp { id: format!("id-{n}"), name: n.to_string() }).collect(),
            clip: Vec::new(),
            now: 0,
            display: None,
        }
    }

    #[test]
    fn display_switch_command_when_configured() {
        let mut c = ctx(&[]);
        assert!(dynamic("écran", &c).iter().all(|e| e.id != "cmd:display"));
        c.display = Some(("24E4".into(), "PC portable".into()));
        for q in ["écran", "ecr", "source", "autre pc", "pc port", "Basculer"] {
            let r = dynamic(q, &c);
            assert_eq!(r.first().map(|e| e.id.as_str()), Some("cmd:display"), "{q}");
        }
        assert_eq!(dynamic("écran", &c)[0].title, "Basculer l'écran 24E4");
        assert_eq!(dynamic("écran", &c)[0].subtitle, "Ce PC ⇄ PC portable");
        assert!(dynamic("ec", &c).iter().all(|e| e.id != "cmd:display"));
        assert!(dynamic("spotify", &c).iter().all(|e| e.id != "cmd:display"));
    }

    fn title(q: &str) -> Option<String> {
        dynamic(q, &ctx(&[])).into_iter().next().map(|e| e.title)
    }

    #[test]
    fn calc_precedence_and_power() {
        assert_eq!(title("2+3*4^2").unwrap(), "= 50");
        assert_eq!(title("(1+2)*3").unwrap(), "= 9");
        assert_eq!(title("2^3^2").unwrap(), "= 512");
        assert_eq!(title("sqrt(16)+pi*0").unwrap(), "= 4");
        assert_eq!(title("7%3").unwrap(), "= 1");
    }

    #[test]
    fn calc_french_decimal() {
        assert_eq!(title("1,5*2").unwrap(), "= 3");
        assert_eq!(title("1/3").unwrap(), "= 0,3333333333");
        assert_eq!(title("0.1+0.2").unwrap(), "= 0,3");
        let e = &dynamic("1,5*2", &ctx(&[]))[0];
        assert_eq!(e.id, "cmd:calc");
        assert_eq!(e.keywords[0], "3");
    }

    #[test]
    fn calc_rejects_plain_numbers_and_words() {
        for q in ["42", "discord", "pi", "-5", "2+", "(2+3", "2 3"] {
            assert!(title(q).is_none(), "{q}");
        }
    }

    #[test]
    fn non_ascii_and_pathological_queries_do_not_panic() {
        let c = ctx(&["Discord"]);
        for q in ["français", "ééé", "kill é", "killé", "kill ", "? é"] {
            let _ = dynamic(q, &c);
        }
        assert_eq!(kill_needle("kill é"), Some("é"));
        assert_eq!(kill_needle("français"), None);
        assert_eq!(kill_needle("ééé"), None);
        assert!(title(&"(".repeat(10_000)).is_none());
        assert!(title(&"-".repeat(10_000)).is_none());
        assert!(title(&"^".repeat(10_000)).is_none());
        assert!(title(&format!("{}1{}+1", "(".repeat(60), ")".repeat(60))).is_some());
        assert!(title(&format!("{}1+1{}", "(".repeat(100), ")".repeat(100))).is_none());
    }

    #[test]
    fn calc_division_by_zero_is_none() {
        assert!(title("1/0").is_none());
        assert!(title("5%0").is_none());
    }

    #[test]
    fn web_prefix_encodes() {
        let e = &dynamic("? rails 8", &ctx(&[]))[0];
        assert_eq!(e.id, "cmd:web");
        assert_eq!(e.keywords[0], "https://www.google.com/search?q=rails%208");
        assert_eq!(e.title, "Rechercher « rails 8 » sur le web");
        assert!(dynamic("? ", &ctx(&[])).is_empty());
        assert!(dynamic("rails", &ctx(&[])).is_empty());
    }

    #[test]
    fn kill_lists_only_matching_running_items() {
        let c = ctx(&["Discord", "Chrome", "Lucent"]);
        let r = dynamic("KILL dis", &c);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, "cmd:kill:id-Discord");
        assert_eq!(r[0].subtitle, "Entrée : quitter · Maj+Entrée : forcer");
        let all = dynamic("kill ", &c);
        assert_eq!(all.len(), 2); // never Lucent itself
        assert!(dynamic("kill", &c).is_empty());
        assert!(dynamic("kill zzz", &c).is_empty());
    }

    #[test]
    fn power_commands_listed_with_confirmation_hint() {
        let e = &dynamic("éteindre", &ctx(&[]))[0];
        assert_eq!(e.id, "cmd:power:shutdown");
        assert_eq!(e.title, "Éteindre");
        assert_eq!(e.subtitle, "Entrée : demander confirmation");
        assert_eq!(e.keywords[0], "shutdown");
        assert_eq!(dynamic("veille", &ctx(&[]))[0].subtitle, "Entrée : exécuter");
        assert_eq!(dynamic("deconnexion", &ctx(&[]))[0].id, "cmd:power:signout");
    }

    #[test]
    fn provider_dispatches_cb_and_emoji_prefixes() {
        let mut c = ctx(&[]);
        let mut ring = super::super::clipboard::Ring::new();
        ring.push("un", 10);
        ring.push("deux", 10);
        c.clip = ring.items().to_vec();
        c.now = 10;
        let t: Vec<_> = dynamic("cb", &c).into_iter().map(|e| e.title).collect();
        assert_eq!(t, ["deux", "un"]);
        assert_eq!(dynamic("cb deu", &c).len(), 1);
        assert_eq!(dynamic(":feu", &c)[0].keywords[0], "🔥");
        assert!(dynamic("cbd", &c).is_empty());
        // History disabled: the store's snapshot is empty, so `cb` lists nothing.
        c.clip = Vec::new();
        assert!(dynamic("cb", &c).is_empty());
    }
}
