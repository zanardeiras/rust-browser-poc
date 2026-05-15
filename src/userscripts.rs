//! Userscripts injetados via WebKitUserContentManager.
//!
//! YouTube serve anúncios em vídeo a partir do mesmo CDN dos vídeos reais
//! (`googlevideo.com`), então bloqueio por URL não funciona sem quebrar o
//! player. A estratégia padrão de adblockers (uBlock, AdGuard) é injetar um
//! script no DOM da página que:
//!   1. Detecta o overlay `.ytp-ad-skip-button` e clica nele;
//!   2. Quando o anúncio NÃO pode ser pulado (`.ytp-ad-player-overlay` sem
//!      botão de skip), avança o vídeo para o final via `video.currentTime`;
//!   3. Silencia o player durante anúncios para evitar áudio agressivo;
//!   4. Esconde banners, masthead e cards promocionais via CSS.
//!
//! O script roda em `DocumentStart` em todos os subframes do youtube.com
//! para também cobrir embeds.

#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]

use webkit2gtk::{
    UserContentManager, UserContentManagerExt, UserScript, UserStyleSheet,
    UserContentInjectedFrames, UserScriptInjectionTime, UserStyleLevel,
};

const YOUTUBE_AD_SKIPPER: &str = r#"
(function() {
    'use strict';
    if (window.__rbpoc_yt_adblock__) return;
    window.__rbpoc_yt_adblock__ = true;

    const log = (...a) => console.log('[rbpoc-yt-adblock]', ...a);

    // Esconde elementos de ad estaticamente via JS (algumas classes só aparecem em runtime).
    function hideAdElements() {
        const sels = [
            'ytd-ad-slot-renderer',
            'ytd-action-companion-ad-renderer',
            'ytd-companion-slot-renderer',
            'ytd-banner-promo-renderer',
            'ytd-statement-banner-renderer',
            'ytd-in-feed-ad-layout-renderer',
            'ytd-ad-inline-playback-meta-block',
            'ytd-promoted-sparkles-web-renderer',
            'ytd-promoted-video-renderer',
            'ytd-display-ad-renderer',
            'ytd-rich-item-renderer:has(ytd-ad-slot-renderer)',
            '#masthead-ad',
            '.ytd-search-pyv-renderer',
            '.ytp-ad-overlay-container',
            '.ytp-ad-image-overlay',
        ];
        for (const s of sels) {
            document.querySelectorAll(s).forEach(el => {
                el.style.setProperty('display', 'none', 'important');
            });
        }
    }

    // Pula anúncios no player: clica skip se possível, senão avança currentTime.
    function skipVideoAds() {
        // 1) Botão "Pular anúncio".
        const skipBtn = document.querySelector(
            '.ytp-ad-skip-button, .ytp-ad-skip-button-modern, .ytp-skip-ad-button'
        );
        if (skipBtn) {
            skipBtn.click();
        }
        // 2) Pop-up "fechar anúncio".
        const closeBtn = document.querySelector('.ytp-ad-overlay-close-button');
        if (closeBtn) closeBtn.click();

        // 3) Detecção de ad em curso → avança o vídeo para perto do fim.
        //    O player do YT exibe a classe `ad-showing` no elemento .html5-video-player.
        const player = document.querySelector('.html5-video-player');
        const video = document.querySelector('video.html5-main-video');
        if (player && video && player.classList.contains('ad-showing')) {
            // Muta para evitar áudio agressivo enquanto pula.
            if (!video.muted) video.muted = true;
            // Acelera ao máximo (browsers limitam, mas 16x é praxe).
            try { video.playbackRate = 16; } catch (_) {}
            // Pula direto para o fim do ad.
            if (isFinite(video.duration) && video.duration > 0) {
                video.currentTime = video.duration;
            }
        } else if (video && video.muted && video.dataset.rbpocMuted === '1') {
            // Restaura mute quando o ad termina.
            video.muted = false;
            delete video.dataset.rbpocMuted;
        } else if (player && video && player.classList.contains('ad-showing') && video.muted) {
            video.dataset.rbpocMuted = '1';
        }
    }

    // Loop principal: leve, ~250ms; MutationObserver pegaria mas YT swap muito DOM.
    setInterval(() => {
        try { skipVideoAds(); hideAdElements(); } catch (e) { /* swallow */ }
    }, 250);

    // Primeira passada imediata quando o DOM aparece.
    if (document.readyState !== 'loading') {
        skipVideoAds(); hideAdElements();
    } else {
        document.addEventListener('DOMContentLoaded', () => {
            skipVideoAds(); hideAdElements();
        }, { once: true });
    }

    log('YouTube ad skipper ativo.');
})();
"#;

const YOUTUBE_AD_CSS: &str = r#"
ytd-ad-slot-renderer,
ytd-action-companion-ad-renderer,
ytd-companion-slot-renderer,
ytd-banner-promo-renderer,
ytd-statement-banner-renderer,
ytd-in-feed-ad-layout-renderer,
ytd-ad-inline-playback-meta-block,
ytd-promoted-sparkles-web-renderer,
ytd-promoted-video-renderer,
ytd-display-ad-renderer,
ytd-rich-item-renderer:has(ytd-ad-slot-renderer),
#masthead-ad,
.ytd-search-pyv-renderer,
.ytp-ad-overlay-container,
.ytp-ad-image-overlay,
.ytp-featured-product,
ytd-merch-shelf-renderer {
    display: none !important;
}
"#;

/// Registra os userscripts/CSS específicos do YouTube no `UserContentManager` da aba.
/// Aplica-se a *.youtube.com e youtube.com (HTTP e HTTPS).
pub fn register_youtube_adblock(ucm: &UserContentManager) {
    let allow_list: &[&str] = &[
        "https://*.youtube.com/*",
        "https://youtube.com/*",
        "https://*.youtube-nocookie.com/*",
    ];
    let block_list: &[&str] = &[];

    let script = UserScript::new(
        YOUTUBE_AD_SKIPPER,
        UserContentInjectedFrames::AllFrames,
        UserScriptInjectionTime::End,
        allow_list,
        block_list,
    );
    ucm.add_script(&script);

    let css = UserStyleSheet::new(
        YOUTUBE_AD_CSS,
        UserContentInjectedFrames::AllFrames,
        UserStyleLevel::User,
        allow_list,
        block_list,
    );
    ucm.add_style_sheet(&css);
}
/// Impede o WebKit de congelar a UI (throttling do requestAnimationFrame)
/// quando a aba não está visível OU quando a janela perde foco.
///
/// Sintoma sem este fix: o vídeo (GStreamer) continua rodando em background,
/// mas a HUD do player YouTube (barra de progresso, controles, tempo) "trava"
/// porque o WebKitGTK suspende o rAF quando a `GtkWindow` perde foco — e o
/// player do YT depende de rAF + eventos `blur/focus` da window para se
/// redesenhar.
///
/// Estratégia (cumulativa, sem efeitos colaterais visíveis):
///   1. `visibilityState/hidden` forçados a visible/false.
///   2. Eventos `visibilitychange`, `blur`, `focus`, `pagehide`, `freeze`
///      bloqueados via capture + stopImmediatePropagation.
///   3. `document.hasFocus()` sempre `true`.
///   4. Polyfill rAF: detecta throttling (callback levou > 100ms) e injeta
///      um fallback baseado em `setTimeout(16ms)` em paralelo, para manter
///      animações da HUD rodando mesmo quando o WebKit suspende rAF nativo.
///   5. AudioContext silencioso (legado, mantém main thread vivo).
///   6. Watchdog: enquanto rAF estiver atrasado, dispara `mousemove` sintético
///      no `.html5-video-player` + `resize` na window — isso força o YT
///      a recompor a HUD assim que a janela volta ao foco.
pub fn register_background_awake(ucm: &UserContentManager) {
    let script_str = r#"
    (function() {
        if (window.__rbpoc_awake__) return;
        window.__rbpoc_awake__ = true;

        // --- 1. visibility forçada -----------------------------------------
        try {
            Object.defineProperty(document, 'visibilityState', { get: () => 'visible', configurable: true });
            Object.defineProperty(document, 'hidden', { get: () => false, configurable: true });
            Object.defineProperty(document, 'webkitHidden', { get: () => false, configurable: true });
        } catch (_) {}

        // --- 2. bloqueio de eventos que sinalizam "fui pra trás" -----------
        const swallow = e => { e.stopImmediatePropagation(); };
        const blocked = ['visibilitychange', 'webkitvisibilitychange',
                         'blur', 'focusout', 'pagehide', 'freeze'];
        for (const ev of blocked) {
            window.addEventListener(ev, swallow, true);
            document.addEventListener(ev, swallow, true);
        }

        // --- 3. hasFocus sempre true ---------------------------------------
        try { document.hasFocus = () => true; } catch (_) {}

        // --- 4. rAF com fallback anti-throttling ---------------------------
        const nativeRAF = window.requestAnimationFrame.bind(window);
        const nativeCAF = window.cancelAnimationFrame.bind(window);
        let lastTick = performance.now();
        let throttled = false;
        const pending = new Map(); // id -> { cb, nativeId, timerId }
        let nextId = 1;

        window.requestAnimationFrame = function(cb) {
            const id = nextId++;
            const entry = { cb, nativeId: 0, timerId: 0, fired: false };
            const wrap = (ts) => {
                if (entry.fired) return;
                entry.fired = true;
                if (entry.timerId) clearTimeout(entry.timerId);
                pending.delete(id);
                const now = performance.now();
                throttled = (now - lastTick) > 100;
                lastTick = now;
                try { cb(ts); } catch (e) { /* swallow */ }
            };
            entry.nativeId = nativeRAF(wrap);
            // Fallback paralelo: se rAF não disparar em 32ms, força via timer.
            entry.timerId = setTimeout(() => wrap(performance.now()), 32);
            pending.set(id, entry);
            return id;
        };
        window.cancelAnimationFrame = function(id) {
            const entry = pending.get(id);
            if (!entry) { try { nativeCAF(id); } catch (_) {} return; }
            entry.fired = true;
            if (entry.nativeId) { try { nativeCAF(entry.nativeId); } catch (_) {} }
            if (entry.timerId) clearTimeout(entry.timerId);
            pending.delete(id);
        };

        // --- 5. AudioContext inaudível (mantém main thread vivo) -----------
        const initAwake = () => {
            if (window.__awake_audio_ctx) return;
            const AudioCtx = window.AudioContext || window.webkitAudioContext;
            if (!AudioCtx) return;
            try {
                window.__awake_audio_ctx = new AudioCtx();
                const osc = window.__awake_audio_ctx.createOscillator();
                const gain = window.__awake_audio_ctx.createGain();
                gain.gain.value = 0.0001;
                osc.connect(gain);
                gain.connect(window.__awake_audio_ctx.destination);
                osc.start();
            } catch (_) {}
        };
        document.addEventListener('mousedown', initAwake, { once: true, capture: true });
        document.addEventListener('keydown',   initAwake, { once: true, capture: true });
        document.addEventListener('touchstart',initAwake, { once: true, capture: true });

        // --- 6. Watchdog: cutuca a HUD do YT quando voltamos do background -
        // A cada 500ms, se detectarmos que rAF voltou após throttle, ou
        // simplesmente periodicamente em páginas YT, disparamos eventos
        // sintéticos que forçam o YT a recompor controles/HUD.
        //
        // Caso especial: vídeo PAUSADO. O GStreamer não emite frames quando
        // o `<video>` está pausado; ao retornar foco, a textura no composite
        // layer fica stale (frame "congelado" preto/borrado). A solução é
        // um seek-to-self (`currentTime = currentTime`) que força o pipeline
        // a re-decodar e o WebKit a re-renderizar o frame.
        const repaintVideos = () => {
            try {
                document.querySelectorAll('video').forEach(v => {
                    // Só atua em vídeos com mídia carregada (readyState >= 2 = HAVE_CURRENT_DATA).
                    if (!v || v.readyState < 2) return;
                    const t = v.currentTime;
                    if (!isFinite(t)) return;
                    if (v.paused) {
                        // Seek-to-self: re-emite o frame atual sem audível "click".
                        try { v.currentTime = t; } catch (_) {}
                    }
                    // Toggle de transform força recomposição da camada GPU.
                    const prev = v.style.transform;
                    v.style.transform = (prev || '') + ' translateZ(0)';
                    requestAnimationFrame(() => { v.style.transform = prev; });
                });
            } catch (_) {}
        };
        // Exposto para o lado Rust acionar via run_javascript no focus-in.
        window.__rbpoc_repaint_videos = repaintVideos;

        let wasThrottled = false;
        setInterval(() => {
            const now = performance.now();
            const gap = now - lastTick;
            // Se rAF parou (gap > 250ms), considera throttled.
            const currentlyThrottled = gap > 250;
            const justResumed = wasThrottled && !currentlyThrottled;
            wasThrottled = currentlyThrottled;

            // Sempre que detectarmos retorno de throttle OU a cada 2s em YT,
            // forçamos o player a redesenhar a HUD.
            const isYT = /youtube\.com|youtube-nocookie\.com/.test(location.hostname);
            if (justResumed || (isYT && Math.random() < 0.05)) {
                try {
                    window.dispatchEvent(new Event('resize'));
                    const player = document.querySelector('.html5-video-player');
                    if (player) {
                        const rect = player.getBoundingClientRect();
                        const x = rect.left + rect.width / 2;
                        const y = rect.top + rect.height / 2;
                        // mousemove cutuca o auto-hide do YT a recompor a HUD
                        player.dispatchEvent(new MouseEvent('mousemove', {
                            bubbles: true, cancelable: true,
                            clientX: x, clientY: y,
                        }));
                    }
                    if (justResumed) repaintVideos();
                } catch (_) {}
            }
        }, 500);
    })();
    "#;

    let script = UserScript::new(
        script_str,
        UserContentInjectedFrames::TopFrame,
        UserScriptInjectionTime::Start,
        &[], // Todas as paginas
        &[],
    );
    ucm.add_script(&script);
}