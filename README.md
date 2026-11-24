# Ferrum Browser

[![CI](https://github.com/carrub-py/ferrum/actions/workflows/ci.yml/badge.svg)](https://github.com/carrub-py/ferrum/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/carrub-py/ferrum?label=Release&color=30d158)](https://github.com/carrub-py/ferrum/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-e5e5ea.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/Windows-10%20%2F%2011%20x64-0078d4.svg)](https://github.com/carrub-py/ferrum/releases/latest)

Navegador web de escritorio para Windows escrito en **Rust** (`tao` + `wry` + **Edge WebView2**), con estética **Apple Obsidian** minimalista en tonos negros puros, protección real de privacidad (`adblock-rust`), sincronización **E2EE Zero-Knowledge** y actualizaciones automáticas verificadas con **SHA-256** desde GitHub Releases.

## Descargar (Windows 10 / 11 x64)

Ve a la página de [**Último Release**](https://github.com/carrub-py/ferrum/releases/latest) y elige tu formato favorito:

1. **Instalador Windows (recomendado):** **`ferrum-browser-setup-x64.exe`** — instala por usuario sin pedir administrador, crea accesos directos, registra en *Aplicaciones predeterminadas* e incluye desinstalador limpio.
2. **Portable (archivo único):** **`ferrum-browser.exe`** (~8 MB) o **`ferrum-browser-windows-x64.zip`** — ejecuta directo sin instalar.
3. **Verificación de integridad opcional:** compara contra **`SHA256SUMS.txt`**:
   ```powershell
   Get-FileHash .\ferrum-browser.exe -Algorithm SHA256
   ```

> **Nota sobre Windows SmartScreen:** al ser un binario open source sin certificado comercial *Code Signing*, Windows puede mostrar *"Windows protegió su PC"* la primera vez. Haz clic en **Más información → Ejecutar de todas formas**.  
> **Requisito:** Microsoft Edge WebView2 Runtime (preinstalado en Windows 11 y la mayoría de Windows 10; el instalador te avisa si falta).

## Características principales

- **Interfaz Apple Obsidian adaptable:** 3 modos de diseño (**Minimalista**, **Compacto 1 fila** y **Vertical estilo Arc**), **grupos de pestañas colapsables con color**, botón `+` pegado a la última pestaña, **arrastre hacia abajo para abrir en nueva ventana**, Vista Dividida con divisor arrastrable, panel lateral redimensionable y asistente de bienvenida en el primer arranque.
- **Omnibox inteligente y ultra rápido:** autocompletado en vivo, **calculadora local y conversor de unidades** (`15% de 240`, `100 km a mi`, `32 c a f`, `2 gb to mb`), filtros instantáneos (`@tabs`, `@bm`, `@hist`), search bangs integrados (`!yt`, `!gh`, `!w`, `!mdn`, `!rs`, `!npm`, `!tr`, `!r`, `!maps`, `!amz`) y atajos personalizados.
- **Escudo de Privacidad nativo (`adblock-rust`) + Popover rápido:** clic en el candado de la barra para controlar Escudo, permisos, zoom, modo lectura/OLED y cookies del sitio actual sin abrir el panel lateral. Incluye EasyList/EasyPrivacy/Cookies (+160.000 reglas), selector visual de elementos (`Alt+E`), limpieza de `utm_*`/`fbclid`/`gclid` y HTTPS-Only.
- **Monitor de pestañas y RAM (`Shift+Esc`):** telemetría real de memoria del árbol de procesos Ferrum + WebView2, hibernación manual/automática y botón *"Hibernar todas las inactivas"* en 1 clic.
- **Espacios con contenedores aislados:** separa sesiones y cookies (General, Trabajo, Estudio o personalizados) con cambio rápido (`Ctrl+Alt+1..9`) + pestañas privadas (Incógnito).
- **Bóveda de contraseñas local cifrada:** cifrado **Argon2id + AES-256-GCM** 100 % en disco, autocompletado bajo demanda (`Ctrl+Shift+L`), importación/exportación CSV (Chrome/Edge/Brave/Bitwarden), borrado seguro en RAM (`zeroize`), auto-bloqueo por inactividad y limpieza automática del portapapeles a los 30 s.
- **Sincronización E2EE Zero-Knowledge + Respaldos locales:** respaldos cifrados `.ferrum-backup` en disco/OneDrive **y sincronización opcional en la nube vía GitHub Gist privado** (cifrado localmente antes de subir; token protegido con **Windows DPAPI**).
- **Actualizaciones automáticas SHA-256 + Recuperación ante fallos:** comprueba nuevos releases en GitHub, verifica el hash `SHA-256` antes de actualizar y recupera sesiones de forma segura en modo hibernado tras cierres inesperados.

## Atajos de teclado

| Atajo | Acción |
| --- | --- |
| `Ctrl+T` / `Ctrl+W` | Nueva pestaña / Cerrar pestaña |
| `Ctrl+Shift+T` | Reabrir pestaña cerrada |
| `Ctrl+N` / `Ctrl+Shift+M` | Nueva ventana / Mover pestaña a nueva ventana |
| `Ctrl+Shift+N` | Nueva pestaña privada (Incógnito) |
| `Ctrl+Alt+1..9` | Cambiar de Espacio de trabajo |
| `Ctrl+L` / `Ctrl+Shift+L` | Enfocar barra / Autocompletar contraseña |
| `Ctrl+K` / `Shift+Esc` | Paleta de comandos / Monitor de pestañas y RAM |
| `Ctrl+\` | Activar o cerrar Vista Dividida |
| `Alt+E` | Selector visual de elementos (Escudo) |
| `Alt+R` / `Alt+D` / `Alt+P` | Modo Lectura / Oscuro OLED / Picture-in-Picture |
| `Ctrl+Shift+S` / `Ctrl+Shift+C` | Captura PNG / Copiar URL limpia |
| `Ctrl+H` / `Ctrl+J` / `Ctrl+F` | Historial / Descargas / Buscar en página |

## Compilar desde el código fuente

Requiere [Rust](https://rustup.rs/) (edición 2024) en Windows 10/11:

```powershell
git clone https://github.com/carrub-py/ferrum.git
cd ferrum
cargo build --release
.\target\release\ferrum-browser.exe
```

### Publicar una nueva versión en GitHub (Release automático)

1. Actualiza `version` en `Cargo.toml` y `CHANGELOG.md`.
2. Haz commit y sube una etiqueta `v*`:
   ```powershell
   git tag -a v0.6.0 -m "Ferrum Browser v0.6.0"
   git push origin main --tags
   ```
3. **GitHub Actions** compilará automáticamente `ferrum-browser.exe`, el portable `ferrum-browser-windows-x64.zip`, el instalador `ferrum-browser-setup-x64.exe` y `SHA256SUMS.txt`, publicando el Release para descarga directa y actualización automática de todos los usuarios.

## Licencia

Distribuido bajo licencia **MIT**. Consulta [LICENSE](LICENSE) y [CHANGELOG.md](CHANGELOG.md).
