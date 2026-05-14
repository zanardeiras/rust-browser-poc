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
/// quando a aba não está visível. Injetamos um micro-contexto de áudio pra "enganar" 
/// o navegador a sempre tratar a aba do YouTube como em atividade alta.
pub fn register_background_awake(ucm: &UserContentManager) {
    let script_str = r#"
    (function() {
        // Redefine document.hidden e document.visibilityState
        Object.defineProperty(document, 'visibilityState', { get: () => 'visible' });
        Object.defineProperty(document, 'hidden', { get: () => false });
        
        // Impede os eventos que notificariam o player que a tela foi pra trás
        window.addEventListener('visibilitychange', e => e.stopImmediatePropagation(), true);
        window.addEventListener('webkitvisibilitychange', e => e.stopImmediatePropagation(), true);
        
        // Dispara um micro canal de áudio inaudível apenas para forçar  
        // a engine do WebKitGTK a manter o Main Thread vivo com timers normais (60fps)
        const initAwake = () => {
            if (window.__awake_audio_ctx) return;
            const AudioCtx = window.AudioContext || window.webkitAudioContext;
            if (!AudioCtx) return;
            window.__awake_audio_ctx = new AudioCtx();
            const osc = window.__awake_audio_ctx.createOscillator();
            const gain = window.__awake_audio_ctx.createGain();
            gain.gain.value = 0.0001; // Quase mudo
            osc.connect(gain);
            gain.connect(window.__awake_audio_ctx.destination);
            osc.start();
        };

        // Inicia no primeiro mousedown ou touch (evita Autoplay blocks do Webkit)
        document.addEventListener('mousedown', initAwake, { once: true });
        document.addEventListener('keydown', initAwake, { once: true });
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