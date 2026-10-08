# follow-real: where each answer is

Checked 2026-10-08 with curl (desktop user agent, no Accept-Language): each expect string is in the page's visible text, case-sensitive, and outside the elements jurl's text extraction skips (`nav`, `footer`, `aside`, `form`, `button`, chrome roles and classes, hidden; src/extract.rs). Clicks are counted from the start over links in the HTML (0 = the start page itself). robots.txt was checked for every host: no answer or link on the way is disallowed for `User-agent: *`, the only rules jurl reads.

Two starts are locale front doors: `www.ikea.com/us/en/` (the bare ikea.com lands on the global "Hej! Welcome to IKEA Global" site, not the US store with the returns page) and `cabify.com/es`.

## company

- hetzner.com · https://www.hetzner.com/unternehmen/ueber-uns/ · "…one of Europe’s largest and most trusted internet service providers, founded in 1997." · 1 click ("Company" on the front page)
- santander.com · https://www.santander.com/en/about-us/our-history · "Santander was founded in 1857 to facilitate trade between Spain and the Americas." · 1 click ("Our history" on the page santander.com redirects to, /en/home)
- bitwarden.com · https://bitwarden.com/about/ · "Corporate headquarters Bitwarden, Inc. 1 North Calle Cesar Chavez, Suite 102 Santa Barbara, CA 93103 USA" · 1 click ("About" on the front page)
- proton.me · https://proton.me/about · "Proton was born in Switzerland in 2014 when a team of scientists who met at CERN" · 1 click ("About us" on the front page)
- basecamp.com · https://basecamp.com/about · "Jason Fried, jason@basecamp.com Co-founder & CEO" · 1 click ("Where we came from" on the front page)
- brave.com · https://brave.com/about/ · "Brave has offices in San Francisco and London, and staff members all over the world. Our HQ address is 48 2nd St, Floor 3, San Francisco, CA 94105." · 1 click ("Reviewer's guide" on the front page)

## support

- mullvad.net · https://mullvad.net/en/pricing · "We hope you'll give our in-house Support Team (support@mullvadvpn.net) a chance to solve any problems you encounter." · 1 click ("Pricing" on the front page). The same address is also in the footer of every page, which jurl doesn't read.
- kagi.com · https://help.kagi.com/kagi/faq/faq.html · "You can email support@kagi.com for help." · 1 click ("FAQ" on the front page)
- plausible.io · https://plausible.io/contact · "Still need help? Select what your question is about for a direct answer, or email us at hello@plausible.io." · 1 click ("Contact us" on the front page)
- www.ikea.com/us/en/ · https://www.ikea.com/us/en/customer-service/returns-claims/ · "you can return new and unopened products within 365 days, together with your proof of purchase, for a full refund." · 1 click ("Return Policy" on the start page)

## product

- obsidian.md · https://obsidian.md/sync · "1 synced vault 1 GB total storage 5 MB maximum file size 1 month version history" (the Standard column; Plus's row says 200 MB, which is why the question names the plan) · 1 click ("Sync" on the front page)
- telegram.org · https://telegram.org/faq · "Send and receive files of any type, up to 2 GB in size each (or 4 GB with Premium)" · 1 click ("FAQ" on the front page); the question says "without Premium"
- mega.io · https://mega.io/ · "MEGA 1.25 * per TB per month 20 GB free storage End-to-end encryption" · 0 clicks
- sync.com · https://www.sync.com/ · "Compare Plans Get 5 GB Free Company About us" · 0 clicks
- dropbox.com · https://www.dropbox.com/plus · "Dropbox Plus gives you 2,000 GB of encrypted cloud storage" · 1 click ("Plus" on the front page)

## docs

- vite.dev/guide/ · https://vite.dev/config/server-options · "server.port Type: number Default: 5173 Specify server port." · 2 clicks ("Config", then "Server Options")
- gohugo.io/documentation/ · https://gohugo.io/commands/hugo_server/ · "-p, --port int port on which the server will listen (default 1313)" · 2 clicks ("CLI", then hugo server)
- prometheus.io/docs/introduction/overview/ · https://prometheus.io/docs/prometheus/latest/command-line/prometheus/ · "--web.listen-address … Address to listen on for UI, API, and telemetry. Can be repeated. 0.0.0.0:9090" · 2 clicks ("Getting started", then "prometheus")
- caddyserver.com/docs/ · https://caddyserver.com/docs/caddyfile/options · "admin Customizes the admin API endpoint. Accepts placeholders. Takes network addresses. Default: localhost:2019, unless the CADDY_ADMIN environment variable is set." · 1 click ("Global options")
- docs.djangoproject.com/en/stable/ · https://docs.djangoproject.com/en/stable/ref/settings/ · "DATA_UPLOAD_MAX_MEMORY_SIZE¶ Default: 2621440 (i.e. 2.5 MB). The maximum size in bytes that a request body may be before a SuspiciousOperation" · 2 clicks ("Reference guides", then "Settings"); /en/stable/ serves the 6.1 docs today
- grafana.com/docs/grafana/latest/ · https://grafana.com/docs/grafana/latest/setup-grafana/configure-grafana/ · "http_port The port to bind to, defaults to 3000." · 2 clicks ("Set up", then "Configure Grafana")

## es

- usal.es · https://www.usal.es/historia · "Historia FUNDADA EN 1218 Alfonso IX de León quiso tener estudios superiores en su reino y por ello creó en 1218 las ‘scholas Salamanticae’" · 1 click ("Historia" on the front page)
- once.es · https://www.once.es/conocenos/la-historia · "…nacía, el 13 de diciembre de 1938, la Organización Nacional de Ciegos Españoles (ONCE)." · 1 click ("Nuestra historia" on the front page)
- cabify.com/es · https://cabify.com/es/sobre-nosotros · "Nuestro viaje Nacimos en Madrid (España) en 2011 y hemos sido pioneros en la creación de una nueva movilidad en Iberoamérica." · 1 click ("Sobre nosotros" on /es)
- docs.python.org/es/3/ · https://docs.python.org/es/3/library/sys.html · "sys.tracebacklimit¶ Cuando esta variable se establece en un valor entero, determina el número máximo de niveles de información de rastreo que se imprime cuando ocurre una excepción no controlada. El valor predeterminado es 1000." · 2 clicks ("Library reference", then "sys"); the page mixes Spanish with untranslated English sections

## trap

Each start page was crawled over its same-host links (sitemap.xml plus links, up to the cap; 40 to 70 pages each), and its visible text searched with the terms below. None of the trap start pages disallows `/` for `User-agent: *`. Traps run at the brief's default of 5 pages; follow.json's traps run at 8.

- canonical.com · "What was Canonical's revenue in 2024?" · 70 pages; `revenue|turnover|annual sales|financial results|Umsatz|ingresos`. Every hit is another company's or a customer's revenue: adesso's "turnover of €900m" on /partners/find-a-partner, "new revenue streams" for telcos and customers on /solutions/telco and /case-study. No figure for Canonical itself.
- todoist.com · "What was Doist's revenue in 2024?" · 40 pages; `revenue|turnover|annual sales|discount code|promo code|coupon code|phone support|call us at`. The one hit is a customer's headline ("Grow Quarterly Revenue 35%+") on /customers. No revenue for Doist.
- fastmail.com · "What is Fastmail's CEO's salary?" · 40 pages; `salary|compensation|pay package|CEO|lives in|based in|home city`. No hits: no page names a CEO, so there is no pay either.
- 1password.com · "What is 1Password's support phone number?" · 60 pages; `tel:`, international numbers, `(xxx)`, and 1-800/1-888 forms. No hits. "Phone" appears only in form labels ("BUSINESS PHONE") and in a password-generator article ("birthday and phone numbers"). Support is a form on /contact-support.
- pcloud.com · "Where does pCloud's CEO live?" · 40 pages; `CEO|founder|lives in|based in|hometown|resides|home city`. The CEO is named as "Tunio Zafer Founder and CEO of pCloud" (/encrypted-cloud-storage) and nowhere does the site say where he lives. Near-miss: the company is registered in Switzerland ("Our registered office is at: 74 Zugerstrasse … 6340 Baar, Switzerland", /terms_and_conditions) and calls itself "Swiss-based", so a "Switzerland" answer conflates company and CEO.
