# Changelog — Ferrum Browser

Todos los cambios notables de **Ferrum Browser** se documentan en este archivo.

## [0.6.1] — 2026-11-27

### Optimización de memoria RAM sin perder fluidez
- **Origen compartido `ferrum://localhost/*` + `--process-per-site`:** todas las vistas de interfaz (`chrome`, `panel`, `overlay`, `newtab` y `spare_tab`) ahora comparten **1 solo proceso renderer (~37 MB)** en lugar de 7 renderers separados (~375 MB), reduciendo el Working Set en reposo en casi **50 %** (`731 MB → 371 MB`) manteniendo apertura instantánea (`0 ms`).
- **Creación bajo demanda (lazy) de WebViews opcionales:** la barra vertical (modo Arc) y el divisor de Vista Dividida solo se crean cuando se activan y se destruyen al cerrarse.
- **Suspensión suave (`ICoreWebView2_3::TrySuspend()` / `Resume()`):** las pestañas ocultas inactivas por ≥ 60 s (sin audio ni carga activa) liberan memoria V8/compositor automáticamente y despiertan en ~1–3 ms sin recargar la página.
- **Flags livianos de WebView2** (desactiva servicios de fondo y funciones extra de Edge sin tocar aceleración GPU) y **medición real de RAM privada (`PrivateWorkingSetSize`)** en el Monitor de pestañas (`Shift+Esc`).

## [0.6.0] — 2026-11-24

### Distribución y calidad nativa en Windows
- **Ícono HD multi-resolución (`16` a `256 px`):** generación supersampled en `build.rs` con PNG `256×256` incrustado en `.ico` y `64×64` RGBA para barra de tareas y Alt+Tab nítidos en pantallas HiDPI.
- **Integración 100 % nativa Win32 (cero `powershell.exe`):** registro de navegador predeterminado vía `RegCreateKeyExW`/`RegSetValueExW` y creación de accesos directos vía COM `IShellLinkW` + `IPersistFile`, evitando falsos positivos de antivirus.
- **Instalador Windows automático (Inno Setup 6):** `ferrum-browser-setup-x64.exe` (sin permisos de administrador, con desinstalador limpio, detección de WebView2 Runtime y firma en `SHA256SUMS.txt`).
- **Asistente de bienvenida (primer arranque):** guía opcional de 3 pasos en Nueva Pestaña para elegir diseño, importar marcadores en 1 clic y registrar en Windows.

### Productividad y navegación diaria
- **Grupos de pestañas colapsables con color:** agrupa, colapsa, cambia de color, arrastra pestañas a grupos y desagrupa en modo Minimalista, Compacto y Vertical; persistidos entre sesiones.
- **Omnibox inteligente:** calculadora local instantánea (`=2+2*5`, `15% de 240`), conversor de unidades (`100 km a mi`, `32 c a f`, `2 gb to mb`), filtros (`@tabs`, `@bm`, `@hist`), bangs integrados (`!yt`, `!gh`, `!w`, `!mdn`, `!rs`, `!npm`, `!tr`, `!r`, `!maps`, `!amz`) y atajos de búsqueda personalizados desde Ajustes.
- **Popover rápido de Escudo y Permisos:** clic en el candado de la barra para pausar escudo, ajustar zoom/lectura/OLED, gestionar permisos del sitio y borrar cookies en 1 clic sin abrir el panel lateral.
- **Monitor de pestañas y RAM (`Shift+Esc`):** telemetría real de procesos Windows (`GetProcessMemoryInfo` sobre el árbol Ferrum + WebView2) y botón *"Hibernar todas las inactivas"*.
- **Gesto de arrastrar pestaña hacia abajo** o `Ctrl+Shift+M` para abrirla en ventana independiente, **cambio rápido de Espacio (`Ctrl+Alt+1..9`)** y **Traducir selección al español** desde el menú contextual.

### Sincronización y estabilidad
- **Sincronización E2EE Zero-Knowledge vía GitHub Gist privado:** cifra marcadores, ajustes, notas, userscripts y bóveda localmente con Argon2id + AES-256-GCM antes de subir; el token personal de GitHub se protege en disco con **Windows DPAPI (`CryptProtectData`)**.
- **Reportes locales de fallos + recuperación segura:** si una sesión se cierra de forma inesperada, Ferrum restaura las pestañas en modo hibernado para evitar bucles de error y guarda diagnósticos sin URLs ni secretos en `crashes/`.
- **Arquitectura modular:** refactorización de `src/browser.rs` en 6 submódulos especializados (`mod`, `tabs`, `layout`, `ipc`, `ui_sync`, `features`).

## [0.5.0] — 2026-11-24
- Actualizador automático verificado obligatoriamente con `SHA-256` contra GitHub Releases y reemplazo atómico en Windows.
- Workflows de CI y Release automático en GitHub Actions (`windows-latest`).
- Borrado seguro de secretos en memoria (`zeroize`), limpieza automática del portapapeles (30 s), `ShellExecuteW` nativo y confirmación en dos pasos para descargas ejecutables.
- Reestructuración responsive de Ajustes, barras y redimensionado exacto en píxeles físicos; eliminación del bloqueo al abrir 10+ pestañas (`ControlFlow::Wait` + modo `--stress-test`).
