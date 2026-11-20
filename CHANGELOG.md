# Changelog

All notable changes to **bbstats** are documented in this file.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.2.0] - 2026-11-21

Safety, reliability, macOS parity, and UI polish release (**v0.2.0**).

### Security, Fixed & Changed

#### English
- **PID Reuse & Process Liveness Safety:** Verified process creation time (`CreationTime` on Windows / start timestamp on Unix) before escalation (`TerminateProcess` / `SIGKILL`) and when registering `ExitWatcher` handles; replaced `GetExitCodeProcess == STILL_ACTIVE` (259) checks with `WaitForSingleObject(..., 0) == WAIT_TIMEOUT` so processes exiting with code 259 are never misclassified as alive.
- **Race-Free Crash Detection:** Added immediate exit-code polling (`poll_exit_code` / `ManagedJobStore::take_exit_code`) plus a 3-second grace window for vanished watched processes so delayed `ExitWatcher` callbacks never silently drop crash alerts or `auto_restart`.
- **FFI Alignment & Pipe Safety:** Migrated `NtQuerySystemInformation` and IP Helper table scratch buffers to 8-byte aligned `Vec<u64>` with `read_unaligned` / `as_chunks` bounds checks; cleared `HANDLE_FLAG_INHERIT` on child capture read pipes, attached `NUL` `hStdInput`, and closed pipes cleanly via RAII `File::from_raw_handle`.
- **Docker Pipe Timeout & API Compatibility:** Switched Docker queries to versionless `/containers/json`, checked HTTP `200` status headers, and bounded Windows named-pipe reads with an atomic in-flight guard and 250 ms timeout; eliminated OS pipe deadlock in `schtasks` / `launchctl` scanning via concurrent bounded stdout draining.
- **Panic & Corrupt Config Protection:** Added ASCII/bounds validation in `parse_hex_color` (`#aö123` safe fallback); backed up corrupt `config.toml` and `labels.toml` to `.bak` and refused silent overwrite; bounded JSON/log reads and added caps/TTLs to `path_cache`, `LogTailCache`, and `project_states`.
- **Secret Redaction & Path Matching:** Extended `sanitize_cmdline` and `strip_shell_redirects` to redact token-only URLs (`https://token@host`), `KEY=SECRET` env assignments, `-H`/`--header` auth tokens, `Bearer` pairs, single quotes, and unspaced `>file` redirects; added directory-boundary checks (`path_str_starts_with_dir`).
- **RFC-8259 JSON & Code Lean-Up:** Replaced 4 hand-rolled JSON parsers across Docker/labels/terminal/manifests with `serde_json`; moved `shot.rs` behind the optional `screenshots` Cargo feature; removed duplicated tray bitmap rasterizer in favor of `LoadImageW` on embedded resource ID 1; dropped 8 unused Windows Cargo features and dead telemetry fields; MSVC-gated manifest linker flags and preserved `assets/bbstats.ico` during normal builds.
- **Complete Bilingual (`EN` / `TR`) Engine & UI:** Threaded `Language` through `EngineCmd::SyncConfigState`, `Snapshot`, `ProjectResolver`, and `KnownJobsScanner` so crash summaries, watchdog notes, fallback titles, and high CPU/RAM alerts render cleanly in the active language; removed personal `.omp/local` folder hardcoding.
- **UI, Multi-Monitor, GPU & Keyboard Fixes:** Scoped `is_fullscreen_active` to `bbstats`'s own monitor; updated DPI on monitor cycling; recovered on `DXGI_ERROR_DEVICE_RESET` and `D2DERR_RECREATE_TARGET` with immediate redraw; guaranteed `PopAxisAlignedClip` via `with_axis_clip`; clipped scrolled hit regions; fixed `O(n)` terminal wrapping, `Alt+P` persistence, `Tab` focus toggle, `Delete`/`Alt+K` `can_kill` guards, Detail `■` job scoping, multi-crash notification order, `git_branch` badges, toast error surfacing, and 16 ms mouse-move redraw throttling.
- **CLI & macOS Parity:** Added `--version`/`--help` and `AttachConsole(ATTACH_PARENT_PROCESS)` on Windows, replaced startup `sleep` waits with `Condvar` first-snapshot synchronization, added startup `MessageBoxW` + rotating `bbstats.log`, implemented `kqueue(EVFILT_PROC)` `ExitWatcher` on macOS, batched `ps`/`lsof` inspection, fixed `ps` `etime == cputime` parsing and macOS `docker_running`/home filters, added `--daemon` mode for LaunchAgent autostart, replaced TCP connect probes with passive `lsof` listening checks, and made `Ctrl+C` responsive within 100 ms.

#### Türkçe
- **PID Yeniden Kullanımı ve Canlılık Güvenliği:** Windows'ta `CreationTime`, Unix'te başlangıç zamanı zorla sonlandırma öncesi ve `ExitWatcher` kaydında tekrar doğrulandı; `STILL_ACTIVE` (259) yerine `WaitForSingleObject(..., 0) == WAIT_TIMEOUT` kontrolüne geçildi.
- **Çökme Tespiti Yarış Koşulu Düzeltmesi:** Listeden kaybolan süreçler için hemen çıkış kodu sorgulama ve 3 saniyelik bekleme penceresi eklenerek geciken `Exited` olaylarında çökme alarmlarının ve `auto_restart` tetiklemelerinin kaybolması önlendi.
- **FFI Hizalama ve Pipe Güvenliği:** `NtQuerySystemInformation` ve IP Helper tamponları 8-bayt hizalı `Vec<u64>`'e taşındı; alt süreç `read_pipe` devralınması kapatıldı, `hStdInput` için `NUL` bağlandı ve RAII `File::from_raw_handle` kullanıldı.
- **Docker Zaman Aşımı ve Pipe Kilitlenmesi:** Docker sorguları sürümsüz `/containers/json` ve HTTP `200` doğrulamasına taşındı; named pipe okumasına 250 ms aşım koruması, `schtasks`/`launchctl` çağrılarına eşzamanlı `stdout` boşaltma eklendi.
- **Panik ve Bozuk TOML Koruması:** `parse_hex_color` ASCII doğrulamasıyla sertleştirildi; bozuk `config.toml` ve `labels.toml` `.bak` olarak yedeklenip üzerine yazım durduruldu; önbellek boyutları (`path_cache`, `LogTailCache`, `project_states`) ve dosya okumaları sınırlandı.
- **Gizli Bilgi Maskeleme ve JSON Sadeleştirme:** Token-only URL'ler, `KEY=SECRET`, `-H`/`--header`, `Bearer`, tek tırnak ve `>dosya` yönlendirmeleri kapsandı; 4 el yazımı JSON parser yerine `serde_json` kullanıldı; `shot.rs` opsiyonel `screenshots` feature arkasına alındı ve `tray.rs` kopya SDF rasterizer kaldırıldı.
- **Tam Çift Dilli (`EN` / `TR`) ve Arayüz İyileştirmeleri:** Çökme özetleri, watchdog notları ve yüksek kaynak alarmları seçili dile geçirildi; çoklu monitörde tam ekran ve DPI güncellendi; `DEVICE_RESET` / `D2DERR_RECREATE_TARGET` kurtarma, kaydırılmış hit-test kırpma, `git_branch` rozetleri, `Alt+P` / `Tab` / `Delete` düzeltmeleri, hata toast bildirimleri, `bbstats.log` ve macOS `kqueue` + `--daemon` desteği eklendi.

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
