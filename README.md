# Ferrum Browser

Navegador web de escritorio para Windows escrito en **Rust** (`tao` + `wry` + **Edge WebView2**), con estética **Apple Obsidian** minimalista en tonos negros puros, protección real de privacidad y actualizaciones automáticas verificadas con **SHA-256** desde GitHub Releases.

## Descargar (Windows 10 / 11 x64)

1. Ve a la página de [**Último Release**](https://github.com/carrub-py/ferrum/releases/latest).
2. Descarga **`ferrum-browser.exe`** (ejecutable portable único, ~8 MB) o **`ferrum-browser-windows-x64.zip`**.
3. Opcionalmente verifica el hash contra **`SHA256SUMS.txt`**:
   ```powershell
   Get-FileHash .\ferrum-browser.exe -Algorithm SHA256
   ```
4. Ejecuta `ferrum-browser.exe`.
   - **Nota sobre Windows SmartScreen**: al ser un binario open source sin certificado comercial *Code Signing*, Windows puede mostrar *"Windows protegió su PC"* la primera vez. Haz clic en **Más información → Ejecutar de todas formas**.
   - **Requisito**: Microsoft Edge WebView2 Runtime (ya viene preinstalado en Windows 11 y la mayoría de equipos con Windows 10).
   - **Actualizaciones automáticas**: Ferrum comprueba nuevas versiones en `github.com/carrub-py/ferrum/releases` al iniciar y desde **Ajustes → Actualizaciones automáticas**, verificando obligatoriamente el hash `SHA-256` antes de reemplazar el ejecutable.

## Características principales

- **Interfaz Apple Noir adaptable**: 3 modos de diseño (**Minimalista**, **Compacto 1 fila** y **Vertical estilo Arc**), botón `+` pegado a la última pestaña, Vista Dividida con divisor arrastrable y panel lateral redimensionable.
- **Escudo de Privacidad nativo (`adblock-rust`)**: bloqueo a nivel de red con EasyList, EasyPrivacy, Peter Lowe y Fanboy Cookies (+160.000 reglas), selector visual de elementos (`Alt+E`), limpieza de `utm_*`/`fbclid`/`gclid`, anti-banners de cookies, HTTPS-Only y limpieza de cookies por sitio.
- **Espacios con contenedores aislados**: separa sesiones y cookies (Personal, Trabajo, Estudio) en perfiles WebView2 independientes + pestañas privadas (Incógnito).
- **Bóveda de contraseñas local cifrada**: cifrado **Argon2id + AES-256-GCM** 100 % en disco (sin nube ni telemetría), autocompletado bajo demanda (`Ctrl+Shift+L`), importación/exportación CSV (Chrome/Edge/Brave/Bitwarden), borrado seguro en RAM (`zeroize`), auto-bloqueo por inactividad y limpieza automática del portapapeles a los 30 s.
- **Respaldo cifrado y sincronización local**: exporta e importa marcadores, ajustes, notas, userscripts y bóveda en un paquete `.ferrum-backup` cifrado (compatible con OneDrive, Dropbox o Syncthing).
- **Productividad diaria**: paleta de comandos (`Ctrl+K`), autocompletado en vivo, hibernación automática y restauración perezosa de sesiones, descargas con pausa/reanudación y aviso en ejecutables, modo Web App (`--app=<url>`), extensiones Chromium MV3 + Userscripts, Picture-in-Picture (`Alt+P`), Modo Lectura (`Alt+R`), Modo Oscuro OLED (`Alt+D`) y capturas PNG (`Ctrl+Shift+S`).

## Atajos de teclado

| Atajo | Acción |
| --- | --- |
| `Ctrl+T` / `Ctrl+W` | Nueva pestaña / Cerrar pestaña |
| `Ctrl+Shift+T` | Reabrir pestaña cerrada |
| `Ctrl+N` / `Ctrl+Shift+N` | Nueva ventana / Pestaña privada |
| `Ctrl+L` / `Ctrl+Shift+L` | Enfocar barra / Autocompletar contraseña |
| `Ctrl+K` | Paleta de comandos |
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

1. Actualiza `version` en `Cargo.toml` (por ejemplo `0.5.1`).
2. Haz commit y sube una etiqueta `v*`:
   ```powershell
   git tag v0.5.1
   git push origin main --tags
   ```
3. **GitHub Actions** compilará automáticamente `ferrum-browser.exe`, generará `ferrum-browser-windows-x64.zip` y `SHA256SUMS.txt`, y publicará el Release para descarga directa y actualización automática de todos los usuarios.

## Licencia

Distribuido bajo licencia **MIT**. Consulta [LICENSE](LICENSE).
