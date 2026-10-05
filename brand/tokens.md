# Leon — Tokens del tema "Leon"

Tabla implementable para el futuro tema "Leon" de la app. Los nombres de
campo son los de `crates/app/src/theme.rs` (`Palette`, `fonts`, `metrics`).
Los valores salen del brand book (no publicado) y del sistema de wuapi
(`wuapi-inbox/crates/app/src/theme.rs`). El acento es el **amarillo ácido**
(decisión del owner) y `warning` es naranja para no chocar con él.

Contrastes: WCAG 2.x, calculados con la misma fórmula que las pruebas de
`theme.rs`.

## 1. Colores

Los neutros son los de wuapi. Los campos nuevos están marcados con **(nuevo)**.

| Campo de `Palette` | Oscuro | Claro | Nota |
|---|---|---|---|
| `background` | `#0A0A0A` | `#FAFAF9` | hoy `#000000` / `#FAFAFA` |
| `surface` | `#141414` | `#FFFFFF` | hoy `#18181B` / `#FFFFFF` |
| `surface_2` | `#1C1C1C` | `#F5F5F4` | fila abierta, control pulsado |
| `border` | `#262626` | `#E7E5E4` | hairline |
| `grid_mark` | `#555555` | `#AAA8A7` | `mix(paper, border, 0.22)` oscuro; `mix(ink, border, 0.28)` claro |
| `guide` | `#333333` | `#D3D1D0` | **nuevo**: la línea más tenue del plano (líneas de cota y marcas de los estados vacíos) |
| `scrim` | `#0A0A0A` al 64% | `#0C0A09` al 46% | igual que hoy en opacidad |
| `elevated_border` | `#5A5A5A` | `#A8A29E` | contorno de lo que flota |
| `text` | `#FAFAF9` | `#0C0A09` | 18,96:1 / 18,96:1 sobre `background` |
| `text_muted` | `#A8A29E` | `#57534E` | 7,85:1 / 7,30:1 |
| `text_faint` | `#8C8580` | `#78716C` | 5,45:1 / 4,59:1 |
| `signal` | `#FFEA00` | `#756600` | amarillo ácido. Sobre `background`: 16,05:1 / 5,50:1. Sobre `surface`: 14,93:1 / 5,74:1. Sobre `surface_2`: 13,81:1 / 5,26:1 |
| `accent_fill` **(nuevo)** | `#FFEA00` | `#FFEA00` | relleno de botón primario e insignias, igual en los dos temas |
| `on_accent_fill` **(nuevo)** | `#0A0A0A` | `#0A0A0A` | 16,05:1 sobre `accent_fill` |
| `primary_fill` | `#FFEA00` | `#FFEA00` | **cambia**: hoy es el color del texto (botón monocromo). Un CTA de acento por pantalla; el resto de botones son contorno |
| `on_primary` | `#0A0A0A` | `#0A0A0A` | |
| `success` | `#4DF688` | `#047857` | sin cambio |
| `warning` | `#FF9500` | `#A06000` | **naranja**: el ámbar `#FFDA5E` quedaba a distancia OKLab 6 del amarillo ácido. 9,00:1 sobre `background` oscuro; 4,82:1 sobre papel y 4,62:1 sobre `surface_2` claro. Distancia CIE76 al acento: 50,1 / 24,6. Siempre con glifo |
| `error` | `#FF5E5E` | `#B91C1C` | sin cambio |
| `info` | `#6EFAFF` | `#0E7490` | sin cambio |
| `logo` | `#FFEA00` | `#0A0A0A` | **cambia**: hoy es monocromo y un test exige `logo != signal`. En oscuro la marca va en el acento; en claro la baldosa va en tinta con cara en acento (`mark-on-light`). Habría que reescribir el test `the_logo_and_the_primary_button_are_monochrome` |
| `agent_claude` | `#D97757` | `#B8583A` | sin cambio |
| `agent_codex` | `#FAFAF9` | `#0C0A09` | monocromo |
| `agent_opencode` | `#F1ECEC` | `#211E1E` | sin cambio |

### Terminal (`Palette::terminal`)

| Campo de `TerminalTheme` | Valor |
|---|---|
| `foreground` | `text` |
| `background` | `background` |
| `cursor` | `signal` |
| `selection` | `mix(background, signal, 0.24)` en Leon (0,38 en Zavu): con el amarillo, 0,38 deja un oliva en el que el texto naranja o rojo baja de 3:1 |
| `ansi` | los 16 colores de Leon (`ANSI_DARK` / `ANSI_LIGHT`); el rojo, verde, amarillo y cian siguen siendo `error`, `success`, `warning`, `info`. El amarillo (`ansi[3]`) es el `warning`: `#FF9500` oscuro, `#A06000` claro; el amarillo brillante (`ansi[11]`) es `#FFB340` / `#8A5200`. Así el texto amarillo de la terminal no se confunde con el cursor `#FFEA00` (distancia CIE76 50,1 / 40,3 en oscuro; 24,6 / 22,0 en claro) |

### Acento: opciones estudiadas

Decidido: **amarillo ácido**. Quedan solo como registro.

| | `signal` oscuro / claro | `accent_fill` | Resultado |
|---|---|---|---|
| lima / oliva | `#D4FF3F` / `#4D7C0F` | `#D4FF3F` | descartada (parece wuapi) |
| dorado | `#F2A31B` / `#9A5B00` | `#F2A31B` | descartada |
| hueso | `#EFE6D2` / `#6B4F1D` | `#EFE6D2` | descartada (un neutro, no una señal) |
| **amarillo ácido** | `#FFEA00` / `#756600` | `#FFEA00` | **elegida**; tinta `#0A0A0A` encima: 16,05:1 |

Los temas de comparación `leon-lime` y `leon-bone` se eliminaron de la app; un
`theme_id` guardado con esos ids vuelve a `leon`.

### Distancias entre los cinco colores con significado (CIE76)

Umbral de las pruebas: **al menos 20 entre cada par**.

| Par | Oscuro | Claro |
|---|---|---|
| acento / success | 74,4 | 51,9 |
| acento / warning | 50,1 | 24,6 |
| acento / error | 97,2 | 63,5 |
| acento / info | 107,4 | 73,8 |
| success / warning | 106,0 | 72,6 |
| success / error | 130,0 | 101,6 |
| success / info | 62,3 | 39,0 |
| warning / error | 53,0 | 41,6 |
| warning / info | 115,3 | 85,7 |
| error / info | 112,2 | 99,8 |

`warning` frente al naranja de Claude Code (`agent_claude`): 43,8 oscuro,
25,6 claro.

## 2. Tipografía

| Token | Hoy | Tema Leon |
|---|---|---|
| `fonts::SANS` | `Space Grotesk` | `Inter` |
| `fonts::MONO` | `Geist Mono` | `JetBrains Mono` |
| `metrics::TEXT_BODY` | 13 px | 13 px |
| `metrics::TEXT_SMALL` | 12 px | 12 px |
| `metrics::TEXT_LABEL` | 10,5 px, mono, semibold | 11 px, mono, semibold, mayúsculas |
| `metrics::TERMINAL_TEXT` | 13 px | 13 px (JetBrains Mono) |
| `metrics::TERMINAL_LINE_HEIGHT` | 1,35 | 1,35 |
| Pesos | | Inter 400, 500, 600; JetBrains Mono 400, 600 |
| Tracking de titulares | | −0,02 em (wordmark −0,035 em) |

Ambas familias son SIL OFL y deben empaquetarse (`assets/ASSETS.md`,
`brand::fonts()`).

## 3. Formas y medidas

| Token | Hoy | Tema Leon | Nota |
|---|---|---|---|
| `metrics::RADIUS` | 2 px | **6 px** | controles, botones, chips, inputs |
| radio de celda y panel | (usa `RADIUS`) | **0** | **nuevo** `RADIUS_CELL`: celdas de grilla y paneles de terminal cuadrados |
| `hairline()` | 1 px | 1 px | sin cambio |
| grosor de borde | 1 px | 1 px | |
| `HEADER_HEIGHT` | 48 px | 48 px | las reglas inferiores se alinean |
| `STATUS_HEIGHT` | 28 px | 28 px | |
| `SIDEBAR_WIDTH` | 320 px | 320 px | |
| `ROW_HEIGHT` | 32 px | 32 px | |
| `CONTROL` | 32 px | 32 px | |
| `INDENT` | 12 px | 12 px | |
| `PALETTE_WIDTH` | 640 px | 640 px | |
| `TAB_BAR_HEIGHT` | 30 px | 30 px | |
| `TERMINAL_PADDING` | 8 px | 8 px | |
| `PROJECT_ICON` / `_LARGE` | 16 / 24 px | 16 / 24 px | |
| Padding de celda (panel) | | 16 px | **nuevo**, ver el brand book §6 |
| Crosshair (`grid_mark`) | | 11 px | brazos de 1 px; `shape.crosshair` |
| Marca de esquina (`lines.tick_length`) | | 8 px | **nuevo**: brazos de 1 px (2 px en el panel con foco) |
| `FOOTER_HEIGHT` | | 41 px | **nuevo**: con `lines.guides`, la barra de estado y las herramientas del árbol miden lo mismo y comparten una regla |
| `SIDEBAR_WIDTH` | 320 px | 320 px | ahora se puede arrastrar entre 220 y 560 px (`SIDEBAR_MIN`, `SIDEBAR_MAX`) |
| Escala de interfaz | 80, 90, 100, 110, 125 % | igual | |

### Líneas (`theme::Lines`)

Los interruptores y pesos del plano, una vez para claro y oscuro. Los colores
son los del tema: `border` (la regla), `grid_mark` (cruces y marcas) y `guide`.

| Token (`[lines]`) | Leon | Zavu | Qué hace |
|---|---|---|---|
| `crosshairs` | `all` | `header` | `off`, `header` (solo bajo la cabecera del árbol) o `all` (cada cruce de reglas y de splits) |
| `corner_ticks` | `true` | `false` | marcas en L en las esquinas de tarjetas, menús y el panel con foco |
| `guides` | `true` | `false` | la regla del pie cruza la ventana: barra de estado y herramientas del árbol a la misma altura |
| `empty_motif` | `true` | `false` | marco de marcas y línea de cota alrededor de los estados vacíos |
| `tick_length` | 8 | 6 | largo de los brazos de una marca |
| `weight` | 1 | 1 | grosor de brazos: 1 o 2 px, igual en todas las escalas |

Banda "tranquila" (contraste contra la página): regla 1.1 a 2:1, `guide` 1.1 a
2:1 y más suave que la cruz, `grid_mark` 1.3 a menos de 3:1. Todas por debajo
del 3:1 de los gráficos y del 4.5:1 del texto.

Sin sombras de color; `elevated` se distingue por `scrim`, `elevated_border`
y, opcionalmente, una regla de 1 px en `signal`.

## 4. Movimiento

Valores propuestos (wuapi no fija números de transición, Leon sí):

| Uso | Duración | Curva |
|---|---|---|
| hover y foco | 120 ms | ease-out |
| abrir la paleta o un menú | 160 ms | ease-out (opacidad 0 a 1, 4 px de desplazamiento) |
| dividir paneles, cambiar de sesión | 0 ms | instantáneo |
| terminal | sin animación propia | |
| parpadeo del logo vivo | 140 ms, dos veces por ciclo de 7 s | heredado |
| mirada del logo vivo | ciclo de 11 s; 4 unidades a los lados, 3 arriba | heredado |
| Movimiento reducido | todo a 0 ms; logo con los ojos abiertos | |

El logo vivo solo se anima en estados vacíos y en "About", nunca en la barra
lateral.

## 5. Marca y recursos

| Recurso | Valor |
|---|---|
| `brand::MARK` (oscuro) | `mark.svg` de la dirección elegida (baldosa acento, cara tinta) |
| `brand::APP_ICON` | `app-icon.svg` (marca al 60% sobre cuadrado `#0A0A0A`) |
| Marca en tema claro | `mark-on-light.svg` (baldosa tinta, cara acento) |
| Marcas de agente | tokens `agent_*`, sin cambios de forma |

## 6. Qué cambia en la app si se adopta esta marca

1. **Fuentes:** Space Grotesk y Geist Mono pasan a Inter y JetBrains Mono;
   hay que empaquetar los archivos y actualizar `brand::fonts()` y
   `fonts::SANS` y `fonts::MONO`.
2. **Acento:** `signal` pasa de violeta `#615FFF` / `#4340C7` a amarillo ácido
   `#FFEA00` / `#756600` (amarillo ácido). Cambian el cursor de la terminal, la
   selección, el anillo de foco y el marcador activo.
3. **Neutros:** `background` oscuro de `#000000` a `#0A0A0A`; `surface` de
   `#18181B` a `#141414`; `border` de `#27272A` a `#262626`; zinc pasa a
   stone (más cálido) en todos los pasos.
4. **Botón primario:** de monocromo (color de texto) a relleno de acento con
   tinta (`primary_fill`, `on_primary`, `accent_fill`, `on_accent_fill`).
5. **Radio:** de 2 px a 6 px en controles (`RADIUS`); `RADIUS_CELL` nuevo en
   0 para paneles y celdas.
6. **Logo:** de monocromo a acento (oscuro) o tinta con cara de acento
   (claro): se retira la regla "el logo nunca es el acento" y su prueba.
7. **Advertencia:** `warning` pasa a naranja (`#FF9500` / `#A06000`) porque
   el amarillo ácido ocupa su lugar, y todo estado lleva también un glifo.
8. **Terminal:** el cursor y la selección siguen el acento nuevo; el
   amarillo ANSI sigue a `warning`.
9. **Pruebas de `theme.rs` a actualizar:** `the_palettes_are_the_brand_values`
   (valores), `the_logo_and_the_primary_button_are_monochrome` (se invierte),
   `the_radius_is_a_single_near_square_token` (el rango deja de ser
   0 a 2,5 px), `the_terminal_cursor_is_the_accent_and_the_selection_a_violet_of_it`
   (la selección ya no es violeta) y la comprobación de `warning` y del
   amarillo ANSI.
10. **El tema Zavu actual** (violeta, cuadrado, Space Grotesk y Geist Mono)
    se conserva como un tema más; las fuentes del tema Zavu seguirían
    empaquetadas mientras el tema exista.
11. **Los tests de contraste existentes** siguen valiendo: con los valores de
    arriba todos los pares de texto cumplen 4,5:1 y los gráficos 3:1
    (verificar `text_faint` claro: 4,59:1, justo por encima).
