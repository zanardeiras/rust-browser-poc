# 🦀 Rust Browser POC (Wry + GTK)

[Português](#português) | [English](#english)

---

## Português

Um navegador web moderno, ultra-leve e focado em performance, construído com **Rust** e o motor **WebKitGTK**. Este projeto foi desenvolvido para ser uma alternativa de baixo consumo de memória ao Chromium, aproveitando a integração nativa com sistemas Linux (Pop!_OS / COSMIC).

### 📊 Performance vs. Chrome
Os testes mostram uma redução drástica no consumo de recursos:

| Cenário | Google Chrome | **Rust Browser (Nosso)** | Economia |
| :--- | :--- | :--- | :--- |
| **Google.com (1 aba)** | ~300 MB | **47 MB** | -84% RAM |
| **YouTube 1080p** | ~1.3 GB | **460 MB** | -65% RAM |

### 🛡️ Privacidade e Segurança
- **AdBlock Nativo**: Bloqueio de anúncios e banners integrado diretamente no motor de renderização.
- **Tracker Blocker**: Impede que rastreadores de terceiros monitorem sua navegação.
- **YouTube Turbo**: Scripts customizados para pular anúncios de vídeo automaticamente.

### ✨ Principais Recursos
- **Motor WebKit Nativo**: Utiliza as bibliotecas do sistema para renderização, reduzindo o overhead.
- **Aceleração de Hardware**: Otimizado para GPUs NVIDIA e drivers Mesa.
- **Persistência Real**: Cookies, cache e logins são salvos localmente e mantidos entre sessões.

### 🚀 Como Executar
```bash
cargo run --release
```

---

## English

A modern, ultra-lightweight, performance-focused web browser built with **Rust** and the **WebKitGTK** engine. This project is a memory-efficient alternative to Chromium, leveraging native Linux integration.

### 📊 Performance vs. Chrome
Benchmarks show a massive reduction in resource usage:

| Scenario | Google Chrome | **Rust Browser (Ours)** | Savings |
| :--- | :--- | :--- | :--- |
| **Google.com (1 tab)** | ~300 MB | **47 MB** | -84% RAM |
| **YouTube 1080p** | ~1.3 GB | **460 MB** | -65% RAM |

### 🛡️ Privacy & Security
- **Native AdBlock**: Ad and banner blocking integrated directly into the engine.
- **Tracker Blocker**: Prevents third-party trackers from monitoring your activity.
- **YouTube Turbo**: Custom scripts to auto-skip video ads.

### ✨ Key Features
- **Native WebKit Engine**: Uses system libraries for rendering to minimize overhead.
- **Hardware Acceleration**: Optimized for NVIDIA GPUs and Mesa drivers.
- **Hard Persistence**: Cookies, cache, and logins are securely saved to disk.

### 🚀 How to Run
```bash
cargo run --release
```

---

## 🛠 Tech Stack
- **Language**: [Rust](https://www.rust-lang.org/)
- **WebView**: [Wry](https://github.com/tauri-apps/wry)
- **UI Toolkit**: [GTK3](https://www.gtk.org/)
- **Acceleration**: NVIDIA Offload + WebKit Compositing
