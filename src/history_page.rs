//! Página interna de histórico (renderizada como data: URL).

use crate::history::{HistoryEntry, HistoryManager};

/// Constrói o HTML completo da página de histórico (UTF-8).
pub fn render_history_html(history: &HistoryManager) -> String {
    let entries = history.load_entries();

    // Agrupa por dia (YYYY-MM-DD calculado a partir do ts_secs).
    let mut groups: Vec<(String, Vec<&HistoryEntry>)> = Vec::new();
    for e in &entries {
        let day = format_day(e.ts_secs);
        match groups.last_mut() {
            Some((d, v)) if *d == day => v.push(e),
            _ => groups.push((day, vec![e])),
        }
    }

    let mut body = String::new();
    body.push_str(&format!(
        r#"<header>
  <h1>Histórico</h1>
  <p class="meta">{} entradas</p>
  <input id="q" type="search" placeholder="Filtrar (pressione / para focar)" autofocus />
</header>"#,
        entries.len()
    ));

    for (day, items) in &groups {
        body.push_str(r#"<section class="day">"#);
        body.push_str(&format!(r#"<h2>{}</h2><ul>"#, html_escape(day)));
        for e in items {
            let time = format_time(e.ts_secs);
            let url_esc = html_escape(&e.url);
            let host = extract_host(&e.url);
            let host_esc = html_escape(&host);
            body.push_str(&format!(
                r#"<li data-host="{host_attr}" data-url="{url_attr}">
  <span class="time">{time}</span>
  <a href="{href}" class="title">{host}</a>
  <span class="url">{url}</span>
</li>"#,
                host_attr = attr_escape(&host),
                url_attr = attr_escape(&e.url),
                time = html_escape(&time),
                href = attr_escape(&e.url),
                host = host_esc,
                url = url_esc,
            ));
        }
        body.push_str("</ul></section>");
    }

    if entries.is_empty() {
        body.push_str(r#"<div class="empty">Nenhuma página visitada ainda.</div>"#);
    }

    format!(
        r#"<!doctype html>
<html lang="pt-BR">
<head>
<meta charset="utf-8" />
<title>Histórico</title>
<style>
  :root {{
    --bg: #0f1115; --panel: #161922; --fg: #e6e8ee; --dim: #8a93a6;
    --accent: #4c9aff; --border: #232737;
  }}
  @media (prefers-color-scheme: light) {{
    :root {{ --bg: #f7f8fa; --panel: #ffffff; --fg: #1a1d24;
             --dim: #6b7280; --accent: #2563eb; --border: #e5e7eb; }}
  }}
  * {{ box-sizing: border-box; }}
  html, body {{ margin: 0; padding: 0; }}
  body {{
    font: 14px/1.55 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto,
          Ubuntu, Cantarell, sans-serif;
    color: var(--fg); background: var(--bg);
    padding: 32px 16px 64px; max-width: 920px; margin: 0 auto;
  }}
  header {{ padding-bottom: 16px; border-bottom: 1px solid var(--border); margin-bottom: 24px; }}
  h1 {{ margin: 0 0 4px; font-size: 24px; font-weight: 600; }}
  .meta {{ margin: 0 0 16px; color: var(--dim); font-size: 12px; }}
  #q {{
    width: 100%; padding: 10px 14px; border-radius: 10px;
    border: 1px solid var(--border); background: var(--panel);
    color: var(--fg); font-size: 14px; outline: none;
  }}
  #q:focus {{ border-color: var(--accent); }}
  section.day {{ margin: 0 0 28px; }}
  h2 {{
    margin: 0 0 8px; font-size: 12px; font-weight: 600;
    text-transform: uppercase; letter-spacing: 0.08em; color: var(--dim);
  }}
  ul {{ list-style: none; padding: 0; margin: 0;
        background: var(--panel); border: 1px solid var(--border);
        border-radius: 12px; overflow: hidden; }}
  li {{ display: grid; grid-template-columns: 64px 220px 1fr; gap: 12px;
        align-items: baseline; padding: 10px 16px;
        border-bottom: 1px solid var(--border); }}
  li:last-child {{ border-bottom: none; }}
  li:hover {{ background: color-mix(in srgb, var(--fg) 6%, transparent); }}
  .time {{ color: var(--dim); font-variant-numeric: tabular-nums; font-size: 12px; }}
  .title {{ color: var(--accent); text-decoration: none; font-weight: 500;
            overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }}
  .title:hover {{ text-decoration: underline; }}
  .url {{ color: var(--dim); font-size: 12px; overflow: hidden;
          text-overflow: ellipsis; white-space: nowrap; font-family: ui-monospace, monospace; }}
  .empty {{ text-align: center; padding: 64px; color: var(--dim); }}
  .hidden {{ display: none !important; }}
</style>
</head>
<body>
{body}
<script>
(function() {{
  const q = document.getElementById('q');
  const items = document.querySelectorAll('li[data-url]');
  function filter() {{
    const term = q.value.toLowerCase().trim();
    items.forEach(li => {{
      const hay = (li.dataset.url + ' ' + li.dataset.host).toLowerCase();
      li.classList.toggle('hidden', term !== '' && !hay.includes(term));
    }});
    document.querySelectorAll('section.day').forEach(sec => {{
      const any = sec.querySelectorAll('li:not(.hidden)').length > 0;
      sec.classList.toggle('hidden', !any);
    }});
  }}
  q.addEventListener('input', filter);
  document.addEventListener('keydown', e => {{
    if (e.key === '/' && document.activeElement !== q) {{
      e.preventDefault(); q.focus(); q.select();
    }}
  }});
}})();
</script>
</body>
</html>"#,
        body = body
    )
}

/// `data:text/html;charset=utf-8,<URL-encoded>` — não exige escrever em disco.
pub fn render_history_data_url(history: &HistoryManager) -> String {
    let html = render_history_html(history);
    let encoded = percent_encode(&html);
    format!("data:text/html;charset=utf-8,{}", encoded)
}

// ---- helpers ----

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn attr_escape(s: &str) -> String {
    // Para uso dentro de atributos HTML — mesmo set do html_escape funciona.
    html_escape(s)
}

/// Percent-encode para data: URL. Só preserva chars ASCII safe; demais viram %HH.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for byte in s.bytes() {
        let safe = matches!(byte,
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' | b'/' | b':' | b'='
            | b'?' | b'@' | b'!' | b'$' | b'\'' | b'(' | b')'
            | b'*' | b'+' | b';' | b','
        );
        if safe {
            out.push(byte as char);
        } else {
            const HEX: &[u8] = b"0123456789ABCDEF";
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0xF) as usize] as char);
        }
    }
    out
}

fn extract_host(url: &str) -> String {
    let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = after_scheme.split('/').next().unwrap_or(after_scheme);
    host.to_string()
}

// === Formatação de data/hora SEM dependência externa ===
//
// Converte unix secs → componentes UTC (algoritmo Howard Hinnant — "civil from days").
// Para um navegador local seria ideal usar timezone do sistema, mas precisaria de
// `chrono`/`time`. Optei por mostrar em UTC e prefixar com offset local aproximado
// via `localtime_r` — sem deps externas.

fn format_day(ts: u64) -> String {
    if ts == 0 { return "Antes de salvarmos datas".into(); }
    let (y, m, d, _, _, _) = local_components(ts);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

fn format_time(ts: u64) -> String {
    if ts == 0 { return "--:--".into(); }
    let (_, _, _, hh, mm, _) = local_components(ts);
    format!("{:02}:{:02}", hh, mm)
}

/// Converte unix secs → (year, month, day, hour, minute, second) no fuso LOCAL.
/// Usa `localtime_r` da libc.
fn local_components(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let t: libc::time_t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm); }
    (
        tm.tm_year + 1900,
        (tm.tm_mon + 1) as u32,
        tm.tm_mday as u32,
        tm.tm_hour as u32,
        tm.tm_min as u32,
        tm.tm_sec as u32,
    )
}
