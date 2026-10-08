# Leon — Voz

Cómo habla Leon. Complementa a `tokens.md`: aquello fija cómo se ve, esto
fija cómo suena. El texto de la app va en inglés; este documento explica las
reglas en español y da los ejemplos en el idioma en que se publican.

## 1. Quién habla

Leon es el león que cuida la manada mientras tú haces otra cosa. Ve todo,
dice poco y no se asusta. Tiene humor seco: el chiste está en contar un hecho
aburrido con total solemnidad, nunca en hacerse el gracioso.

| Es | No es |
|---|---|
| seco, breve, seguro | entusiasta, efusivo, con signos de admiración en fila |
| cómplice del que trabaja | mascota que pide atención |
| exacto: nombra el archivo, el comando, el número | vago: "algo salió mal" |
| orgulloso de la manada | burlón con el usuario o con el agente |

## 2. Dos registros

**Interfaz (toda la app).** Frases llanas, sin broma. Un botón dice lo que
hace, un error dice qué pasó y qué hacer. Aquí la voz es solo sobriedad.

**La guarida (`The Den`).** El único lugar donde Leon narra. Es un homenaje al
cuadro de texto de los juegos de criaturas de 8 bits: frases cortas, sujeto
en mayúsculas, presente o pasado simple, un hecho por línea. El humor sale de
tratar un `grep` como si fuera un combate.

La guarida nunca miente para hacer el chiste. Cada línea cómica corresponde a
un hecho real, y el dato exacto (archivo, comando, estado) está siempre a un
gesto de distancia: al pasar el cursor o al seleccionar al cachorro.

## 3. Vocabulario de la guarida

| Cosa real | En la guarida |
|---|---|
| la vista | the Den |
| todas las sesiones vivas | the pride |
| una sesión de agente | a lion (lleva el nombre de la sesión, EN MAYÚSCULAS) |
| un sub-agente | a little one; sale de un huevo junto a quien lo lanzó |
| llamadas a herramientas hechas | `Lv.` (el nivel es el número de herramientas usadas, no un invento) |
| editar, escribir | teclear en su escritorio de piedra (the desks) |
| leer, buscar | hurgar en el estante (the shelf) |
| ejecutar comandos | la máquina grande (the rack) |
| web | el telescopio del mirador (the lookout) |
| planear | mirar fijamente la pizarra (the board) |
| esperando al usuario | de pie en la entrada, con `!` |
| pide permiso | "A wild PERMISSION PROMPT appeared." |
| inactivo en el shell | en la roca al sol, con café |
| dormido | `Zzz`, a veces sobre el teclado |
| salió con error | fainted |
| salió bien | went home |

## 4. Plantillas

Se eligen de forma determinista (misma sesión y mismo evento, misma frase), se
rotan para no repetir y nunca superan dos líneas de 34 caracteres.

| Hecho | Ejemplos |
|---|---|
| empieza a editar | `MOSS used EDIT on shell.rs!` · `MOSS is typing furiously at shell.rs.` |
| empieza a leer | `MOSS is sniffing around keys.rs.` |
| busca | `MOSS empties the shelf looking for "Overlay"...` |
| ejecuta un comando | `MOSS used CARGO TEST! Waiting for it to land...` |
| el comando falla | `It's not very effective...` |
| el comando pasa | `It's super effective!` (solo si de verdad pasó) |
| lanza un sub-agente | `MOSS sent out EXPLORE!` · `An egg is hatching...` |
| vuelve un sub-agente | `EXPLORE came back with a report.` |
| pide permiso | `A wild PERMISSION PROMPT appeared.` · `MOSS is staring at you.` |
| terminó el turno | `MOSS is done. MOSS wants a new order.` |
| lleva rato callado | `MOSS is thinking very hard. Or napping.` |
| se durmió | `MOSS fell asleep.` |
| salió con error | `MOSS fainted! (exit 101)` |
| nadie en la guarida | `The den is quiet. Too quiet.` |
| muchos trabajando | `The pride is busy. You may go get coffee.` |
| resumen de un turno pasado (historial del feed) | `MOSS used 14 tools.` · `MOSS used a tool.` |
| el feed, sin nada que contar | `Nothing to tell yet.` |
| el feed de un león que aún no dijo nada | `MOSS has not said a word yet.` |
| el feed de una sesión sin transcript | `MOSS keeps its thoughts to itself.` (seguido del hecho llano) |

Lo que el agente le escribió al usuario **no** pasa por estas plantillas: el
feed lo muestra tal cual, como habla (`MOSS said`), sin reescribirlo. Con el
narrador apagado cada plantilla tiene su gemela llana (`Running \`cargo test\``,
`Used 14 tools.`), sin nombre y sin broma.

## 5. Reglas

1. **Un hecho, una línea.** Sin adjetivos de relleno, sin emojis.
2. **Nombres propios en mayúsculas**, comandos y archivos tal cual se escriben.
3. **Un solo signo de admiración**, y solo cuando algo ocurre de verdad.
4. **Nunca culpar.** El agente "fainted", no "failed you". El usuario nunca es
   el chiste.
5. **Nunca inventar progreso.** Si no se sabe qué hace el agente (sesión remota,
   agente sin transcripción), la línea lo dice: `MOSS is doing something
   mysterious.`
6. **Lo urgente no es broma.** Un permiso pendiente o un fallo llevan además su
   color y su glifo de estado, igual que en el resto de la app.
7. **Homenaje, no copia.** Se toma la cadencia del género; no se usan nombres,
   criaturas, tipografías, sonidos ni arte de ninguna franquicia.
8. **Movimiento reducido:** poses fijas y texto completo de una vez; la
   información es la misma.

## 6. Arte de la guarida

- Pixel art de artista, en archivos PNG embebidos y listados en
  `crates/app/assets/ASSETS.md`. La base son los recursos abiertos de Pixel
  Agents (MIT) y los personajes MetroCity de JIK-A-4 (CC0), modificados: los
  cuerpos se conservan y la cabeza, la melena y la cola son de león.
  Rejilla de 16 px por baldosa, escalado entero, sin suavizado.
- La escena tiene su propia paleta cálida y no cambia con el tema; el marco,
  la lista de la manada y el cuadro de texto sí usan los tokens.
- La guarida es una oficina amueblada: escritorios con computadora, estantes,
  una pizarra, un rincón de máquinas, un mirador, una sala con sofá y café, un
  tapete en la entrada y un nido para los huevos.
- Los leones van en dos patas. Teclear en el escritorio es la imagen por
  defecto de "está trabajando" y tiene que leerse de un vistazo.
- La melena de cada león se tiñe con el color de su agente (`agent_*`); el
  pelaje es siempre de león. `signal` y los cuatro colores de estado, con su
  glifo, marcan la selección y lo urgente.
- Marcas de esquina y regla de 1 px en el cuadro de texto, como el resto del
  plano de Leon.
