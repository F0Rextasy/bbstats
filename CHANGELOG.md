# Changelog

All notable changes to **bbstats** are documented in this file.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.1.0] - 2026-11-18

Initial open-source release of **bbstats** (`v0.1.0`) — a lightweight, zero-runtime-dependency Dynamic Island process and developer project telemetry monitor for Windows 10/11 and macOS (`arm64` / `x86_64`).

### Added

#### English
- **Dynamic Island Native UI & Spring Engine:** Built directly on Rust + Direct3D11 + Direct2D + DirectWrite + WinRT Composition (`WS_EX_TOOLWINDOW` with Acrylic glass blur, `PerMonitorV2` DPI awareness, fullscreen auto-hide, top-center and right-edge rail docking).
- **Smooth Envelope Morph & Visual Polish:** Single-envelope non-tearing resize animations (`SPRING_MORPH`), rounded-clip layer masking, non-overlapping crossfade schedule, cursor-tracking top-rim specular reflection, status-aware top ambient glow (green/amber/crimson), Catmull-Rom cubic Bezier sparklines, and staggered row/tab micro-transitions.
- **Project & Job Inference Engine:** Resolves workspace manifests (`package.json`, `Cargo.toml`, `pyproject.toml`, `go.mod`, `.git`, Docker Compose), collapses wrapper trees (`cmd`, `conhost`, `venv` shims) into clean readable job rows, and applies noise filtering for shells/IDE sidecars.
- **Live Telemetry, Multi-Port Chips & Log Inspection:** Tracks per-project CPU/RAM curves, bound TCP port chips (`:8080↗`, left-click open / right-click copy), interactive Detail tabs with wrapping rows, and lazy log/pipe/console/state-JSON tailing with severity highlighting.
- **Crash Alerts & Bounded Auto-Restart:** Detects unexpected non-zero exits, preserves crashed job cards in the list with one-click `↻ Restart`, sends system tray balloon alerts, and supports bounded `auto_restart` rules (max 3 attempts per 10 minutes).
- **Bilingual (`EN` / `TR`) Localization:** Live in-app **English** / **Türkçe** toggle in Settings (`language = "en"` | `"tr"` in `config.toml`), localized tray context menu, notifications, and bilingual `README.md` + `README.tr.md` with dedicated `EN`/`TR` screenshot galleries.
- **Security & Secret Redaction:** Automatic CLI masking of tokens, API keys, and basic-auth URLs (`--token=***`, `--api-key ***`), plus a strict process blacklist preventing accidental termination of OS/self processes.
- **Branding & CLI Tools:** Procedural obsidian **`bb`** monogram `.ico` / Win32 resources (`build.rs`), system tray icon, SVG logo, `--dump-snapshot`, and privacy-clean demo `--dump-shot --lang en|tr` (plus `--live`).
- **Cross-Platform macOS Support (`arm64` / `x86_64`):** Native process/memory/CPU collector (`ps`, `sysctl`, `vm_stat`), `lsof` listening TCP/UDP port & CWD inspection, Docker Unix socket API queries, POSIX `SIGTERM`→`SIGKILL` subtree termination, `~/Library/Application Support/bbstats/` config & rules, `~/Library/LaunchAgents` autostart, and interactive ANSI Dynamic Island terminal HUD (`bbstats --watch` / `--status` / `--dump-snapshot`).
- **Open-Source Infrastructure:** MIT `LICENSE`, multi-OS GitHub Actions CI & tag-based Release packaging workflows (`.github/workflows/ci.yml`, `release.yml`) for both Windows (`x86_64`) and macOS (`arm64` / `x86_64`).

#### Türkçe
- **Dynamic Island Yerel Arayüz ve Yay Motoru:** Saf Rust + Direct3D11 + Direct2D + DirectWrite + WinRT Composition üzerinde sıfır ek runtime ile çalışır (`PerMonitorV2` DPI, Acrylic cam bulanıklığı, tam ekranda otomatik gizleme, üst-orta veya sağ dikey ray yerleşimi).
- **Pürüzsüz Zarf Morph ve Cam Parlaması:** Yırtılmasız zarf yeniden boyutlandırma animasyonu, köşe klip maskesi, çakışmasız sıralı metin geçişi, imleç takipli üst kenar cam parlaması, duruma duyarlı üst ortam parıltısı (yeşil/amber/kırmızı) ve Catmull-Rom kübik Bezier sparkline grafikleri.
- **Proje ve İş Gruplama Motoru:** Çalışma dizini köklerini (`package.json`, `Cargo.toml`, `pyproject.toml`, `go.mod`, `.git`, Docker Compose) otomatik çözümler; `cmd`, `conhost`, `venv` gibi sarmalayıcı süreçleri tek anlamlı iş satırı altında toplar.
- **Canlı Telemetri, Çoklu Port ve Log Takibi:** Proje CPU/RAM geçmişi, tıklanabilir `:port↗` çipleri, çok satırlı Detay iş sekmeleri ve `INFO`/`WARN`/`ERROR` renklendirmeli log/pipe/konsol çıktısı izleme.
- **Çökme Bildirimleri ve Sınırlı Otomatik Yeniden Başlatma:** Sıfır dışı çıkışları yakalar, çöken işi listede tutup tek tık `↻ Restart` sunar, sistem tepsisi balonu gönderir ve sınırlı `auto_restart` (10 dakikada en fazla 3 deneme) politikasını destekler.
- **Çoklu Dil (`EN` / `TR`) Desteği:** Ayarlar panelinden anlık **English** / **Türkçe** dil değişimi, yerelleştirilmiş tepsi menüsü ve ayrı `EN`/`TR` ekran görüntülü dokümantasyon.
