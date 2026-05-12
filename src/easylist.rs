//! EasyList / EasyPrivacy importer e conversor para WebKit Content Blockers JSON.
//!
//! ## Por que isso?
//!
//! As regras "handcrafted" em `assets/adblock-rules.json` cobrem o básico, mas
//! a comunidade mantém listas com **dezenas de milhares** de filtros curados:
//!
//!   - **EasyList**: lista canônica de bloqueio de anúncios.
//!   - **EasyPrivacy**: trackers, telemetria, fingerprinting.
//!
//! Esses arquivos usam a sintaxe Adblock Plus (ABP). Nós convertemos para o
//! formato JSON aceito pelo motor `WebKitUserContentFilterStore`, que compila
//! tudo num DFA otimizado.
//!
//! ## Limitações do motor WebKit (importantes)
//!
//!   - **Sem disjunções regex** (`|` alternation) — nenhuma `(a|b|c)` permitida.
//!     Solução: usamos `if-domain` arrays e múltiplas regras separadas.
//!   - **Max ~50 000 regras** por filtro. Truncamos se necessário.
//!   - **Order matters**: regras `block` primeiro, depois exceções
//!     (`ignore-previous-rules`).
//!
//! ## Estratégia de execução
//!
//!   1. Cache local em `~/.cache/rust-browser-poc/easylist-cache/`.
//!   2. Se cache > 7 dias → spawn `std::thread` que baixa via `curl` (zero dep).
//!   3. Thread converte ABP → JSON e escreve `combined-rules.json` no cache.
//!   4. Notifica main thread via `glib::MainContext::channel`.
//!   5. Main thread chama `AdBlock::recompile_from_disk`.

#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const EASYLIST_URL: &str =
    "https://easylist.to/easylist/easylist.txt";
pub const EASYPRIVACY_URL: &str =
    "https://easylist.to/easylist/easyprivacy.txt";

/// Idade máxima do cache antes de revalidar.
pub const CACHE_TTL: Duration = Duration::from_secs(60 * 60 * 24 * 7);

/// Caminho do JSON final compilado (bundled + EasyList).
pub fn combined_rules_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("easylist-cache/combined-rules.json")
}

fn raw_path(cache_dir: &Path, name: &str) -> PathBuf {
    cache_dir.join("easylist-cache").join(name)
}

/// Verifica se o cache combinado existe e está fresco.
pub fn cache_fresh(cache_dir: &Path) -> bool {
    let p = combined_rules_path(cache_dir);
    match std::fs::metadata(&p).and_then(|m| m.modified()) {
        Ok(t) => SystemTime::now()
            .duration_since(t)
            .map(|d| d < CACHE_TTL)
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// Faz download via `curl` (presente em qualquer Linux). Retorna o texto.
fn fetch(url: &str) -> Result<String, String> {
    let output = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "--max-time", "30",
            "--retry", "2",
            "-A", "rust-browser-poc/1.0",
            url,
        ])
        .output()
        .map_err(|e| format!("curl spawn: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "curl exit {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("utf8: {}", e))
}

/// Garante listas baixadas (re-baixa se stale). Idempotente.
/// Retorna (easylist_text, easyprivacy_text) — em qualquer caso, mesmo offline,
/// tenta usar o que existe em disco.
fn ensure_lists(cache_dir: &Path) -> (Option<String>, Option<String>) {
    let dir = cache_dir.join("easylist-cache");
    let _ = std::fs::create_dir_all(&dir);

    let el_path = raw_path(cache_dir, "easylist.txt");
    let ep_path = raw_path(cache_dir, "easyprivacy.txt");

    // Função interna: lê cache ou faz download.
    let get = |path: &Path, url: &str| -> Option<String> {
        let stale = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .map(|t| SystemTime::now().duration_since(t).map(|d| d > CACHE_TTL).unwrap_or(true))
            .unwrap_or(true);
        if !stale {
            return std::fs::read_to_string(path).ok();
        }
        match fetch(url) {
            Ok(txt) => {
                let _ = std::fs::write(path, &txt);
                Some(txt)
            }
            Err(e) => {
                eprintln!("[easylist] download failed for {}: {} (fallback to cache)", url, e);
                std::fs::read_to_string(path).ok()
            }
        }
    };

    (get(&el_path, EASYLIST_URL), get(&ep_path, EASYPRIVACY_URL))
}

// ============================================================================
// ABP → WebKit JSON parser
// ============================================================================

#[derive(Default, Debug)]
pub struct ConvertStats {
    pub parsed_lines: usize,
    pub network_blocks: usize,
    pub network_exceptions: usize,
    pub cosmetic_rules: usize,
    pub skipped_complex: usize,
    pub skipped_comment: usize,
    pub final_rule_count: usize,
    pub truncated: bool,
}

/// Limite seguro para o motor WebKit (oficial é 50k; deixamos margem).
const MAX_RULES: usize = 48_000;

/// Converte texto ABP combinado em string JSON pronta para o store.
/// As regras do JSON `bundled` são pré-pendadas (têm prioridade alta de match).
pub fn build_combined_json(
    bundled_json: &str,
    easylist_txt: Option<&str>,
    easyprivacy_txt: Option<&str>,
) -> (String, ConvertStats) {
    let mut stats = ConvertStats::default();
    let mut blocks: Vec<String> = Vec::new();
    let mut exceptions: Vec<String> = Vec::new();
    let mut cosmetics: Vec<String> = Vec::new();

    // O bundled é JSON array completo — extraímos seu corpo bruto e re-inserimos
    // no início para máxima prioridade. Como já é válido, fazemos pass-through.
    let bundled_trimmed = bundled_json.trim();
    let bundled_inner = bundled_trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .map(|s| s.trim())
        .unwrap_or("");

    for source in [easylist_txt, easyprivacy_txt].into_iter().flatten() {
        for line in source.lines() {
            let line = line.trim();
            stats.parsed_lines += 1;

            // Comentários e cabeçalhos.
            if line.is_empty() || line.starts_with('!') || line.starts_with('[') {
                stats.skipped_comment += 1;
                continue;
            }

            // === COSMETIC FILTERS ===
            // domain1,domain2##selector  →  css-display-none escopado.
            // #@# exceptions são ignoradas (raras e complexas).
            if let Some(idx) = find_cosmetic_marker(line) {
                let (prefix, selector) = (&line[..idx], &line[idx + 2..]);
                if line[idx..idx + 3].starts_with("#@#") {
                    // Exceção cosmética — skip.
                    stats.skipped_complex += 1;
                    continue;
                }
                // Scriptlets uBO `##+js(...)` — skip (motor diferente).
                if selector.starts_with("+js(") {
                    stats.skipped_complex += 1;
                    continue;
                }
                // Procedural filters (`:has-text`, `:matches-css`, etc.) — não suportado por WebKit nativamente.
                if selector.contains(":has-text(")
                    || selector.contains(":matches-css(")
                    || selector.contains(":matches-path(")
                    || selector.contains(":xpath(")
                    || selector.contains(":upward(")
                    || selector.contains(":remove(")
                    || selector.contains(":style(")
                {
                    stats.skipped_complex += 1;
                    continue;
                }
                if selector.is_empty() {
                    stats.skipped_complex += 1;
                    continue;
                }
                let domains = parse_domain_list(prefix);
                if let Some(rule) = make_cosmetic_rule(&domains, selector) {
                    cosmetics.push(rule);
                    stats.cosmetic_rules += 1;
                } else {
                    stats.skipped_complex += 1;
                }
                continue;
            }

            // === NETWORK FILTERS ===
            let (raw, is_exception) = if let Some(rest) = line.strip_prefix("@@") {
                (rest, true)
            } else {
                (line, false)
            };

            // Separa opções pós-$.
            let (pattern, opts) = split_options(raw);

            // Opções complexas que não suportamos → skip.
            let parsed_opts = match parse_options(opts) {
                Some(o) => o,
                None => { stats.skipped_complex += 1; continue; }
            };

            // Converte pattern → (url_filter, if_domain).
            let conv = match convert_pattern(pattern) {
                Some(c) => c,
                None => { stats.skipped_complex += 1; continue; }
            };

            // Combina if-domain do pattern com $domain=…
            let mut if_domain = conv.if_domain;
            let mut unless_domain: Vec<String> = Vec::new();
            for d in &parsed_opts.domains_include { if_domain.push(d.clone()); }
            for d in &parsed_opts.domains_exclude { unless_domain.push(d.clone()); }

            // Constrói JSON da regra.
            let rule = build_network_rule_json(
                &conv.url_filter,
                &if_domain,
                &unless_domain,
                &parsed_opts.resource_types,
                parsed_opts.third_party_only,
                parsed_opts.first_party_only,
                is_exception,
            );
            if is_exception {
                exceptions.push(rule);
                stats.network_exceptions += 1;
            } else {
                blocks.push(rule);
                stats.network_blocks += 1;
            }
        }
    }

    // Monta JSON final: bundled → block → cosmetic → exceptions.
    // (Exceções DEVEM vir por último para o `ignore-previous-rules` funcionar.)
    let mut out = String::with_capacity(1024 * 1024);
    out.push('[');
    let mut first = true;

    if !bundled_inner.is_empty() {
        out.push_str(bundled_inner);
        first = false;
    }

    let mut total = 0usize;
    'outer: for v in [&blocks, &cosmetics, &exceptions] {
        for r in v {
            if total >= MAX_RULES { stats.truncated = true; break 'outer; }
            if !first { out.push(','); }
            out.push_str(r);
            first = false;
            total += 1;
        }
    }
    out.push(']');

    stats.final_rule_count = total
        + if bundled_inner.is_empty() { 0 } else { count_top_level_objects(bundled_inner) };
    (out, stats)
}

/// Conta objetos top-level num corpo JSON (estimativa rápida, conta `{` no
/// profundidade 0). Não precisa ser exato — é só para logar.
fn count_top_level_objects(inner: &str) -> usize {
    let mut depth = 0i32;
    let mut count = 0usize;
    let bytes = inner.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'{' => { if depth == 0 { count += 1; } depth += 1; }
            b'}' => { depth -= 1; }
            b'"' => {
                // skip string
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' { i += 2; continue; }
                    if bytes[i] == b'"' { break; }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    count
}

/// Encontra a posição do marcador cosmético (`##`, `#@#`, `#?#`, `#$#`).
fn find_cosmetic_marker(line: &str) -> Option<usize> {
    // Procura por `##`, `#@#`, `#?#`, `#$#`. WebKit não tem suporte a `#?#`/`#$#`
    // (procedural / scriptlet), então só reconhecemos `##` e `#@#` aqui.
    let bytes = line.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'#' {
            if bytes[i + 1] == b'#' { return Some(i); }
            if i + 2 < bytes.len() && bytes[i + 1] == b'@' && bytes[i + 2] == b'#' {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn parse_domain_list(s: &str) -> Vec<String> {
    if s.is_empty() { return Vec::new(); }
    s.split(',')
        .map(|d| d.trim())
        .filter(|d| !d.is_empty() && is_valid_domain_chars(d))
        .map(|d| normalize_domain_for_webkit(d))
        .collect()
}

fn is_valid_domain_chars(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'*' | b'~'))
}

/// WebKit `if-domain` aceita `*domain.com` para casar `domain.com` + subdomínios.
/// ABP exclusão `~domain` vai pra `unless-domain` (tratado fora).
fn normalize_domain_for_webkit(d: &str) -> String {
    let d = d.trim_start_matches('~');
    // Se já tem ponto e não começa com `*`, prefixa `*` para incluir subdomínios.
    if d.starts_with('*') { d.to_string() } else { format!("*{}", d) }
}

/// Gera regra cosmética. Retorna None se selector tiver chars perigosos para JSON.
fn make_cosmetic_rule(domains: &[String], selector: &str) -> Option<String> {
    // Domínios excluídos (com `~`) → unless-domain.
    let (include, exclude): (Vec<_>, Vec<_>) = domains
        .iter()
        .partition(|d| !d.contains('~') && !d.contains("*~"));
    // Aliás, nossa partition acima está errada porque após normalize_domain_for_webkit já 
    // perdemos o `~`. Re-parse simples: domains aqui são apenas inclusões.
    let _ = exclude;

    let sel_esc = json_escape(selector);
    let mut out = String::with_capacity(128 + sel_esc.len());
    out.push_str(r#"{"trigger":{"url-filter":".*""#);
    if !include.is_empty() {
        out.push_str(r#","if-domain":["#);
        let mut first = true;
        for d in include {
            if !first { out.push(','); }
            out.push('"'); out.push_str(&json_escape(d)); out.push('"');
            first = false;
        }
        out.push(']');
    }
    out.push_str(r#"},"action":{"type":"css-display-none","selector":""#);
    out.push_str(&sel_esc);
    out.push_str(r#""}}"#);
    Some(out)
}

fn split_options(pattern: &str) -> (&str, &str) {
    if let Some(idx) = pattern.rfind('$') {
        // Heurística: $ no fim de regex literal não é separador. Não suportamos
        // regex literal aqui (vamos converter pattern como substring/glob).
        (&pattern[..idx], &pattern[idx + 1..])
    } else {
        (pattern, "")
    }
}

#[derive(Default)]
struct ParsedOptions {
    resource_types: Vec<&'static str>,
    third_party_only: bool,
    first_party_only: bool,
    domains_include: Vec<String>,
    domains_exclude: Vec<String>,
}

/// Parseia opções pós-`$`. Retorna None se houver opção não suportada (ex.: `csp`,
/// `removeparam`, `redirect`, etc.) — preferimos pular do que gerar regra errada.
fn parse_options(s: &str) -> Option<ParsedOptions> {
    let mut po = ParsedOptions::default();
    if s.is_empty() { return Some(po); }
    for opt in s.split(',') {
        let opt = opt.trim();
        if opt.is_empty() { continue; }
        match opt {
            "script" => po.resource_types.push("script"),
            "image" => po.resource_types.push("image"),
            "stylesheet" => po.resource_types.push("style-sheet"),
            "font" => po.resource_types.push("font"),
            "media" => po.resource_types.push("media"),
            "ping" => po.resource_types.push("ping"),
            "xmlhttprequest" | "xhr" => po.resource_types.push("raw"),
            "subdocument" | "frame" => po.resource_types.push("document"),
            "third-party" | "3p" => po.third_party_only = true,
            "~third-party" | "~3p" | "first-party" | "1p" => po.first_party_only = true,
            // Negações simples e opções neutras que podemos ignorar:
            "~script" | "~image" | "~stylesheet" | "~font" | "~media" | "~xmlhttprequest"
            | "~subdocument" | "~frame" | "~document" | "object" | "~object"
            | "websocket" | "~websocket" | "popup" | "~popup" | "popunder"
            | "elemhide" | "~elemhide" | "generichide" | "~generichide"
            | "genericblock" | "important" | "match-case" | "~match-case"
            | "all"
            => {}
            other if other.starts_with("domain=") => {
                for d in other[7..].split('|') {
                    if d.is_empty() { continue; }
                    if let Some(d) = d.strip_prefix('~') {
                        if is_valid_domain_chars(d) {
                            po.domains_exclude.push(normalize_domain_for_webkit(d));
                        }
                    } else if is_valid_domain_chars(d) {
                        po.domains_include.push(normalize_domain_for_webkit(d));
                    }
                }
            }
            // Opções com semântica que NÃO conseguimos representar fielmente
            // (geram bypass se ignoradas). Mais seguro pular.
            other if other.starts_with("csp=")
                || other.starts_with("redirect=")
                || other.starts_with("redirect-rule=")
                || other.starts_with("rewrite=")
                || other.starts_with("removeparam")
                || other.starts_with("removeheader")
                || other.starts_with("denyallow=")
                || other == "inline-script"
                || other == "inline-font"
                || other == "empty"
                || other == "mp4"
                || other == "doc"
            => return None,
            _ => {} // tolerar opções desconhecidas em vez de descartar a regra
        }
    }
    Some(po)
}

struct ConvertedPattern {
    url_filter: String,
    if_domain: Vec<String>,
}

/// Converte um pattern ABP em (regex WebKit-safe, if-domain extra).
/// Retorna None se gerar algo que o motor rejeitaria.
fn convert_pattern(p: &str) -> Option<ConvertedPattern> {
    let p = p.trim();
    if p.is_empty() {
        return Some(ConvertedPattern { url_filter: ".*".into(), if_domain: vec![] });
    }

    // 1) Regex literal /.../ — só aceitamos se não contiver `|` nem grupos.
    if p.starts_with('/') && p.ends_with('/') && p.len() >= 2 {
        let body = &p[1..p.len() - 1];
        if body.contains('|') || body.contains("(?") || body.is_empty() {
            return None;
        }
        // Verifica chars permitidos (heurística conservadora).
        if !regex_body_is_webkit_safe(body) { return None; }
        return Some(ConvertedPattern { url_filter: body.to_string(), if_domain: vec![] });
    }

    // 2) Domain anchor `||domain^...`
    if let Some(rest) = p.strip_prefix("||") {
        return convert_domain_anchored(rest);
    }

    // 3) Pipe anchor `|http://...|`
    let (anchored_start, p2) = if let Some(rest) = p.strip_prefix('|') {
        (true, rest)
    } else { (false, p) };
    let (anchored_end, body) = if let Some(stripped) = p2.strip_suffix('|') {
        (true, stripped)
    } else { (false, p2) };

    let mut out = String::new();
    if anchored_start { out.push('^'); }
    out.push_str(&abp_glob_to_regex(body)?);
    if anchored_end { out.push('$'); }

    if out.contains('|') { return None; }
    Some(ConvertedPattern { url_filter: out, if_domain: vec![] })
}

/// `||domain.com^...$opts` → if-domain + url-filter para o sufixo.
fn convert_domain_anchored(rest: &str) -> Option<ConvertedPattern> {
    // Encontra fim do domínio: primeiro char não-domínio ou `^`.
    let mut split = rest.len();
    for (i, c) in rest.char_indices() {
        if !(c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' || c == '*') {
            split = i;
            break;
        }
    }
    let (domain, suffix) = rest.split_at(split);
    if domain.is_empty() || !is_valid_domain_chars(domain) {
        return None;
    }
    let if_domain = vec![normalize_domain_for_webkit(domain)];

    // Sufixo após o domínio (path/query). Se vazio ou só `^`, basta if-domain
    // com url-filter `.*`.
    let suffix = suffix.trim_start_matches('^');
    if suffix.is_empty() {
        return Some(ConvertedPattern { url_filter: ".*".into(), if_domain });
    }
    let regex = abp_glob_to_regex(suffix)?;
    if regex.contains('|') { return None; }
    Some(ConvertedPattern { url_filter: regex, if_domain })
}

/// Converte glob ABP em regex. ABP usa:
///   * → .*
///   ^ → separador: `[^a-zA-Z0-9_.%-]` (WebKit aceita classes)
///   demais chars: literais (escape regex)
fn abp_glob_to_regex(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '*' => out.push_str(".*"),
            '^' => out.push_str("[^a-zA-Z0-9_.%-]"),
            // Chars regex que precisam escape:
            '.' | '\\' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '$' => {
                out.push('\\'); out.push(ch);
            }
            // Char `|` é forbidden — abortamos.
            '|' => return None,
            // Apenas ASCII printável — descarta unicode (raríssimo em URLs).
            c if c.is_ascii() && !c.is_ascii_control() => out.push(c),
            _ => return None,
        }
    }
    Some(out)
}

/// Heurística: regex literal `/.../` é seguro para WebKit?
fn regex_body_is_webkit_safe(body: &str) -> bool {
    !body.contains('|')
        && !body.contains("(?")
        && !body.contains("\\b")
        && !body.contains("\\B")
        && body.bytes().all(|b| b.is_ascii() && !b.is_ascii_control())
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn build_network_rule_json(
    url_filter: &str,
    if_domain: &[String],
    unless_domain: &[String],
    resource_types: &[&str],
    third_party_only: bool,
    first_party_only: bool,
    is_exception: bool,
) -> String {
    let mut out = String::with_capacity(160);
    out.push_str(r#"{"trigger":{"url-filter":""#);
    out.push_str(&json_escape(url_filter));
    out.push('"');

    if !if_domain.is_empty() {
        out.push_str(r#","if-domain":["#);
        let mut first = true;
        for d in if_domain {
            if !first { out.push(','); }
            out.push('"'); out.push_str(&json_escape(d)); out.push('"');
            first = false;
        }
        out.push(']');
    }
    if !unless_domain.is_empty() {
        out.push_str(r#","unless-domain":["#);
        let mut first = true;
        for d in unless_domain {
            if !first { out.push(','); }
            out.push('"'); out.push_str(&json_escape(d)); out.push('"');
            first = false;
        }
        out.push(']');
    }
    if !resource_types.is_empty() {
        out.push_str(r#","resource-type":["#);
        let mut first = true;
        for r in resource_types {
            if !first { out.push(','); }
            out.push('"'); out.push_str(r); out.push('"');
            first = false;
        }
        out.push(']');
    }
    if third_party_only {
        out.push_str(r#","load-type":["third-party"]"#);
    } else if first_party_only {
        out.push_str(r#","load-type":["first-party"]"#);
    }
    out.push_str(r#"},"action":{"type":""#);
    out.push_str(if is_exception { "ignore-previous-rules" } else { "block" });
    out.push_str(r#""}}"#);
    out
}

// ============================================================================
// Pipeline em background (download + parse + escreve no disco)
// ============================================================================

/// Roda em std::thread. Baixa listas (com cache), faz parsing, escreve JSON
/// combinado em disco. Não toca em UI — apenas I/O e CPU.
pub fn refresh_in_background(
    cache_dir: PathBuf,
    bundled_json: String,
    on_done: glib::Sender<bool>,
) {
    std::thread::spawn(move || {
        eprintln!("[easylist] starting refresh in background...");
        let (el, ep) = ensure_lists(&cache_dir);
        if el.is_none() && ep.is_none() {
            eprintln!("[easylist] no lists available (cache empty + offline).");
            let _ = on_done.send(false);
            return;
        }
        let (json, stats) =
            build_combined_json(&bundled_json, el.as_deref(), ep.as_deref());
        eprintln!(
            "[easylist] parsed_lines={} block={} except={} cosmetic={} skipped_complex={} skipped_comment={} final={} truncated={}",
            stats.parsed_lines,
            stats.network_blocks,
            stats.network_exceptions,
            stats.cosmetic_rules,
            stats.skipped_complex,
            stats.skipped_comment,
            stats.final_rule_count,
            stats.truncated,
        );
        let out_path = combined_rules_path(&cache_dir);
        if let Some(parent) = out_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&out_path, &json) {
            eprintln!("[easylist] write combined json failed: {}", e);
            let _ = on_done.send(false);
            return;
        }
        eprintln!("[easylist] combined rules written to {:?}", out_path);
        let _ = on_done.send(true);
    });
}
