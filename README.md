# jurl

curl, pero lee la página por ti. [Jev](https://docs.typesafe.ai/introduction) elige los párrafos que importan; [Clef](https://developers.cloudflare.com/workers-ai/models/clef-flash/) mira las imágenes.

Todo lo que imprime está literalmente en la página: los modelos solo **eligen**, nunca escriben.

```
url ─fetch─▶ HTML/markdown ─▶ bloques + imágenes ─▶ Jev (1 request, 1 Noul por bloque) ─▶ stdout
                                      └─ --vision ─▶ Clef-flash (1 imagen por request, en paralelo)
```

## Uso

```sh
jurl https://en.wikipedia.org/wiki/William_Stanley_Jevons      # ficha: título, tipo de página, bloques clave
jurl -n 5 example.com/post                                      # solo los 5 mejores bloques
jurl --all example.com/docs                                     # todo lo que pase el umbral
jurl --image example.com/post | xargs -n1 curl -sO              # imágenes de contenido, mejor primero
jurl --vision example.com/post                                  # igual, pero Clef mira los píxeles
jurl -q "how do I install it?" github.com/BurntSushi/ripgrep  # solo lo que responde a la pregunta
jurl --code github.com/BurntSushi/ripgrep                       # bloques de código: ejemplos, comandos
jurl --links -n 10 news.ycombinator.com                         # enlaces que merece la pena seguir
jurl --json -t example.com                                      # JSON con probabilidades + tiempos en stderr
```

| Flag | |
| --- | --- |
| `-q, --ask "…"` | Elige lo que responde a la pregunta (combina con `--code`, `--links`, `--image`) |
| `-c, --code` | Solo bloques de código |
| `-l, --links` | URLs de enlaces de contenido, mejor primero (sin navegación, login, redes, legal) |
| `-i, --image` | URLs de imágenes de contenido (Jev juzga alt, caption, nombre y tamaño) |
| `--vision` | Clef-flash clasifica los píxeles de las 8 mejores según Jev y se promedia con Jev; el resto conserva la nota de Jev |
| `-n, --max N` | Máximo de resultados (12 bloques, 5 con `--ask`, 8 de código, 20 enlaces, imágenes sin límite) |
| `-a, --all` | Sin máximo |
| `--threshold P` | Probabilidad mínima (0.5) |
| `--json` | Salida JSON con `p` por bloque/imagen |
| `-t, --timing` | Tiempo por fase en stderr |

## Keys

Del entorno, o de `~/.config/jurl/env` / `./.env` (`KEY=valor`):

- `TYPESAFE_API_KEY` — siempre
- `CLOUDFLARE_ACCOUNT_ID`, `CLOUDFLARE_AI_TOKEN` — solo `--vision`

## Latencia medida (2026-10-04, mediana de 3 pasadas)

| Página | fetch | Jev | total |
| --- | --- | --- | --- |
| blog.cloudflare.com (markdown nativo) | 122 ms | 440 ms | 563 ms |
| Wikipedia (2 requests en paralelo) | 247 ms | 416 ms | 672 ms |
| GitHub README | 242 ms | 373 ms | 647 ms |
| Rust book | 104 ms | 504 ms | 605 ms |
| Hacker News | 732 ms | 245 ms | 982 ms |

`--vision` añade ~1,2–1,5 s (descarga + Clef, en paralelo).

## Decisiones

- **Extracción propia con `scraper`**, sin readability: se quita nav/footer/aside/scripts y se trocea en bloques. Jev se encarga del resto del boilerplate.
- `Accept: text/markdown` primero: los sitios con *Markdown for Agents* de Cloudflare se saltan el parseo HTML.
- Una Noul por bloque en **un** request; si la página no cabe (~60k chars de state o 120 preguntas) se parte en requests paralelos.
- Los headings no se preguntan: se imprimen si su sección conserva algún bloque.
- Clef responde mejor a *qué es* (Choice: foto, gráfica, logo, avatar…) que a *si importa*; se suman las clases de contenido y se promedia con el juicio de contexto de Jev.

## Pendiente

- `--render` para páginas JS (hoy: error claro "no readable content").
