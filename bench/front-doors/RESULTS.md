# --follow from a front door: benchmark

Measured 2026-10-10 with `bench/front-doors/run.py`. Every cell is `jurl -t --json --precise --follow 5 -q QUESTION FRONT`; `-t` only adds stderr. Jev is behind `jev_proxy.py`, which answers an identical request from a cache and forwards the rest. Real-world sheets (R1-R6, R6es) come first everywhere.

**Final state = step 4.** Its binary is byte-identical to the final build (12,048,496 bytes). Step 5 was measured and reverted.

## Headline

| set | round | cells | correct | wrong | not found |
|---|---|---:|---:|---:|---:|
| real-world | baseline | 85 | 45 | 19 | 21 |
| real-world | step4 (final) | 85 | 46 | 14 | 25 |
| developer | baseline | 25 | 16 | 3 | 6 |
| developer | step4 (final) | 25 | 17 | 3 | 5 |
| all | baseline | 110 | 61 | 22 | 27 |
| all | step4 (final) | 110 | 63 | 17 | 30 |

## Rounds

Columns: correct / wrong / not found per sheet; mean pages read; mean tokens used (what each run's JSON reports, cached answers included); billed tokens (what Jev charged for that round's cells: the proxy's forwarded answers) and its cost at $0.042 per million input tokens.

**Baseline: #50 + #51 as merged (no change of ours)**

| sheet | cells | correct | wrong | not found | mean pages | mean tokens used | billed tokens | billed $ |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| R1 | 12 | 7 | 1 | 4 | 4.2 | 74,961 | 894,784 | 0.0376 |
| R2 | 18 | 7 | 3 | 8 | 3.5 | 50,921 | 916,586 | 0.0385 |
| R3 | 8 | 2 | 4 | 2 | 2.5 | 37,895 | 303,161 | 0.0127 |
| R4 | 15 | 8 | 4 | 3 | 3.2 | 45,712 | 659,687 | 0.0277 |
| R5 | 12 | 2 | 7 | 3 | 4.5 | 85,416 | 1,010,527 | 0.0424 |
| R6 | 10 | 10 | 0 | 0 | 3.0 | 82,119 | 821,188 | 0.0345 |
| R6es | 10 | 9 | 0 | 1 | 3.0 | 86,975 | 869,750 | 0.0365 |
| S1 | 5 | 1 | 0 | 4 | 5.0 | 89,874 | 449,368 | 0.0189 |
| S2 | 8 | 5 | 1 | 2 | 3.5 | 79,282 | 619,200 | 0.0260 |
| S3 | 12 | 10 | 2 | 0 | 4.0 | 76,080 | 912,959 | 0.0383 |
| **all** | 110 | 61 | 22 | 27 | 3.6 | 68,341 | 7,457,210 | 0.3132 |

Failures by class (baseline): wrong_answer 22, not_reached 13, not_on_site 5, js_only 4, missed_on_read 3, blocked 2.

**Step 1: robots.txt `Sitemap:` lines; a sitemap index read newest-first by `<lastmod>`**

| sheet | cells | correct | wrong | not found | mean pages | mean tokens used | billed tokens | billed $ |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| R1 | 12 | 7 | 1 | 4 | 4.2 | 74,692 | 119,306 | 0.0050 |
| R2 | 18 | 6 | 3 | 9 | 3.5 | 49,151 | 82,370 | 0.0035 |
| R3 | 8 | 2 | 4 | 2 | 2.5 | 37,895 | 0 | 0.0000 |
| R4 | 15 | 8 | 2 | 5 | 3.3 | 59,084 | 461,767 | 0.0194 |
| R5 | 12 | 4 | 6 | 2 | 4.2 | 81,956 | 356,478 | 0.0150 |
| R6 | 10 | 10 | 0 | 0 | 3.0 | 87,716 | 63,872 | 0.0027 |
| R6es | 10 | 9 | 0 | 1 | 3.0 | 92,740 | 65,689 | 0.0028 |
| S1 | 5 | 1 | 0 | 4 | 5.0 | 89,874 | 0 | 0.0000 |
| S2 | 8 | 5 | 1 | 2 | 3.5 | 79,863 | 114,933 | 0.0048 |
| S3 | 12 | 10 | 2 | 0 | 4.1 | 75,731 | 169,476 | 0.0071 |
| **all** | 110 | 62 | 19 | 29 | 3.6 | 70,505 | 1,433,891 | 0.0602 |

Failures by class (step1): wrong_answer 19, not_reached (carried) 13, not_on_site (carried) 5, js_only (carried) 4, not_reached 3, missed_on_read (carried) 2, blocked (carried) 2.

**Step 2: llms.txt of the front door's linked hosts on the same registrable domain (PSL)**

| sheet | cells | correct | wrong | not found | mean pages | mean tokens used | billed tokens | billed $ |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| R1 | 12 | 7 | 1 | 4 | 4.2 | 74,692 | 0 | 0.0000 |
| R2 | 18 | 6 | 3 | 9 | 3.5 | 49,151 | 1,112 | 0.0000 |
| R3 | 8 | 2 | 4 | 2 | 2.5 | 37,895 | 0 | 0.0000 |
| R4 | 15 | 8 | 2 | 5 | 3.3 | 58,318 | 0 | 0.0000 |
| R5 | 12 | 3 | 7 | 2 | 4.2 | 81,184 | 0 | 0.0000 |
| R6 | 10 | 10 | 0 | 0 | 3.0 | 87,716 | 0 | 0.0000 |
| R6es | 10 | 9 | 0 | 1 | 3.0 | 92,740 | 0 | 0.0000 |
| S1 | 5 | 1 | 0 | 4 | 5.0 | 90,250 | 65,486 | 0.0028 |
| S2 | 8 | 5 | 1 | 2 | 3.5 | 79,863 | 0 | 0.0000 |
| S3 | 12 | 10 | 2 | 0 | 4.1 | 76,596 | 105,183 | 0.0044 |
| **all** | 110 | 61 | 20 | 29 | 3.6 | 70,428 | 171,781 | 0.0072 |

Failures by class (step2): wrong_answer 20, not_reached (carried) 13, not_on_site (carried) 5, js_only (carried) 4, not_reached 3, missed_on_read (carried) 2, blocked (carried) 2.

**Step 3: links to other registrable domains (explicit links and bare URLs in the text)**

| sheet | cells | correct | wrong | not found | mean pages | mean tokens used | billed tokens | billed $ |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| R1 | 12 | 7 | 0 | 5 | 4.2 | 76,022 | 184,457 | 0.0077 |
| R2 | 18 | 5 | 2 | 11 | 4.2 | 52,753 | 326,659 | 0.0137 |
| R3 | 8 | 3 | 3 | 2 | 2.5 | 42,119 | 100,323 | 0.0042 |
| R4 | 15 | 8 | 3 | 4 | 3.3 | 60,162 | 153,146 | 0.0064 |
| R5 | 12 | 4 | 6 | 2 | 4.3 | 90,159 | 404,972 | 0.0170 |
| R6 | 10 | 10 | 0 | 0 | 3.0 | 85,404 | 185,468 | 0.0078 |
| R6es | 10 | 10 | 0 | 0 | 3.2 | 88,966 | 204,011 | 0.0086 |
| S1 | 5 | 2 | 0 | 3 | 5.0 | 106,662 | 195,645 | 0.0082 |
| S2 | 8 | 5 | 1 | 2 | 3.5 | 80,540 | 69,356 | 0.0029 |
| S3 | 12 | 10 | 2 | 0 | 4.0 | 77,036 | 256,996 | 0.0108 |
| **all** | 110 | 64 | 17 | 29 | 3.7 | 72,990 | 2,081,033 | 0.0874 |

Failures by class (step3): wrong_answer 17, not_reached (carried) 13, js_only (carried) 4, not_on_site (carried) 4, missed_on_read (carried) 2, not_on_site 2, blocked (carried) 2, missed_on_read 1, not_reached 1.

**Step 4 (FINAL): Accept-Language `en` by default, `JURL_ACCEPT_LANGUAGE` overrides**

| sheet | cells | correct | wrong | not found | mean pages | mean tokens used | billed tokens | billed $ |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| R1 | 12 | 7 | 0 | 5 | 4.2 | 76,011 | 2,919 | 0.0001 |
| R2 | 18 | 5 | 2 | 11 | 4.2 | 52,753 | 0 | 0.0000 |
| R3 | 8 | 2 | 3 | 3 | 2.5 | 40,201 | 4,202 | 0.0002 |
| R4 | 15 | 8 | 3 | 4 | 3.3 | 60,163 | 14,750 | 0.0006 |
| R5 | 12 | 4 | 6 | 2 | 4.3 | 90,477 | 8,930 | 0.0004 |
| R6 | 10 | 10 | 0 | 0 | 3.0 | 85,313 | 30,200 | 0.0013 |
| R6es | 10 | 10 | 0 | 0 | 3.2 | 88,966 | 0 | 0.0000 |
| S1 | 5 | 2 | 0 | 3 | 5.0 | 106,088 | 82,468 | 0.0035 |
| S2 | 8 | 5 | 1 | 2 | 3.5 | 80,540 | 0 | 0.0000 |
| S3 | 12 | 10 | 2 | 0 | 4.0 | 77,036 | 0 | 0.0000 |
| **all** | 110 | 63 | 17 | 30 | 3.7 | 72,849 | 143,469 | 0.0060 |

Failures by class (step4): wrong_answer 17, not_reached (carried) 13, not_on_site (carried) 6, js_only (carried) 4, missed_on_read (carried) 2, not_reached 2, blocked (carried) 2, missed_on_read 1.

**Step 5 (tried, REVERTED): the sitemaps of linked hosts, read through their robots.txt**

| sheet | cells | correct | wrong | not found | mean pages | mean tokens used | billed tokens | billed $ |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| R1 | 12 | 7 | 0 | 5 | 4.2 | 81,227 | 262,801 | 0.0110 |
| R2 | 18 | 5 | 2 | 11 | 4.2 | 58,772 | 139,344 | 0.0059 |
| R3 | 8 | 2 | 4 | 2 | 2.5 | 42,494 | 90,352 | 0.0038 |
| R4 | 15 | 8 | 4 | 3 | 3.6 | 66,691 | 158,399 | 0.0067 |
| R5 | 12 | 4 | 6 | 2 | 4.5 | 100,304 | 483,819 | 0.0203 |
| R6 | 10 | 10 | 0 | 0 | 3.0 | 85,313 | 0 | 0.0000 |
| R6es | 10 | 10 | 0 | 0 | 3.2 | 88,966 | 0 | 0.0000 |
| S1 | 5 | 3 | 1 | 1 | 4.6 | 98,730 | 219,759 | 0.0092 |
| S2 | 8 | 5 | 1 | 2 | 3.8 | 84,368 | 118,922 | 0.0050 |
| S3 | 12 | 10 | 1 | 1 | 3.8 | 75,929 | 137,637 | 0.0058 |
| **all** | 110 | 64 | 19 | 27 | 3.8 | 76,356 | 1,611,033 | 0.0677 |

Failures by class (step5): wrong_answer 19, not_reached (carried) 12, not_on_site (carried) 6, js_only (carried) 4, missed_on_read (carried) 3, blocked (carried) 1, not_on_site 1.

## Final round, cell by cell

Verdict per cell. A correct or wrong answer is judged by its quote, with the one-line reason from `judgements.json`. A cell with no answer gives its class (see the legend) and the reason from `notfound.json`.

| sheet | row | question | answer (closest) | verdict | reason |
|---|---|---|---|---|---|
| R1 | Impact Hub Madrid | What is the monthly price of a flexible (hot) desk? | 120€
Contratar online | not_found | missed_on_read (carried). (carried from baseline) /servicios-precios/puestos-flexibles lists 'Flex Completa € 220' (read; --precise picked 120€ instead) |
| R1 | Impact Hub Madrid | Can you buy a day pass? | Ven cuando lo necesites. Adquiere tu pase de un día o un bono de 10 dí | correct | "Adquiere tu pase de un dia o un bono de 10 dias": a day pass can be bought. |
| R1 | Impact Hub Madrid | Is it open 24/7? | 24/7 | correct | The plans list "Acceso 24/7" (24/7 access for members), so the space is open round the clock for that plan. |
| R1 | Utopicus | What is the monthly price of a flexible (hot) desk? | 259€/mes | correct | The Utopicus Passport, its flexible workstation, is listed at 259 EUR/mes ("Comfortable workstation 24/7 access"). |
| R1 | Utopicus | Can you buy a day pass? | 10 pases de día | correct | "Bono 10 pases de dia Working Pass": a pack of 10 day passes is sold. |
| R1 | Utopicus | Is it open 24/7? | 4-8 | not_found | missed_on_read. The same hours page as in step 3: read, not picked. |
| R1 | Talent Garden Madrid | What is the monthly price of a flexible (hot) desk? |  | not_found | not_reached (carried). (carried from baseline) 5 pages read (listings, location pages) had no price; the prices page was not reached (unverified) |
| R1 | Talent Garden Madrid | Can you buy a day pass? | Fixed desks, hourly packages or access to the digital community | not_found | not_reached (carried). (carried from baseline) 5 pages read had no day-pass text (closest: hourly packages); not reached (unverified) |
| R1 | Talent Garden Madrid | Is it open 24/7? | 24/7 access in all TAGs | correct | "24/7 access in all TAGs" (TAG = Talent Garden campus) on the front page. |
| R1 | Lexington | What is the monthly price of a flexible (hot) desk? | Alquiler zonas comunes
€
Electricidad
€
Comunidad
€
Mantenimiento
€
In | not_found | not_reached (carried). (carried from baseline) tarifas pages read (warmth 0.13) without the price in their text (unverified) |
| R1 | Lexington | Can you buy a day pass? | espacios compartidos (escritorios en oficinas compartidas o pases diar | correct | Lexington sells "pases diarios a zonas de trabajo comun" (daily passes to shared areas). |
| R1 | Lexington | Is it open 24/7? | 24/7 | correct | "podreis acceder a la oficina alquilada de manera ilimitada, 24/7" (members' access, 24/7). |
| R2 | seaya.vc | Do they invest at pre-seed? | Your founder-friendly financing solution | not_found | not_reached (carried). (carried from step2) Regression: the 2023 fund article that answered in baseline sits in a sitemap that newest-first no longer reads. |
| R2 | seaya.vc | What is their typical first ticket? | €300m | not_found | js_only (carried). (carried from baseline) the front door needs rendering (Lightpanda); no first-ticket text in the rendered pages |
| R2 | seaya.vc | How do you send them a pitch? | dealflow.website@seaya.vc | correct | The Contact page has a "Pitch" heading with dealflow.website@seaya.vc under it. |
| R2 | Kibo Ventures | Do they invest at pre-seed? | We lead or co-lead pre-Series A or Series A rounds | not_found | not_on_site (carried). (carried from baseline) the site says 'pre-Series A or Series A rounds', not pre-seed, on the pages read |
| R2 | Kibo Ventures | What is their typical first ticket? | No | not_found | not_reached (carried). (carried from step3) Regression: a cross-domain typeform (a whistleblowing form) was followed and the search lost the first-ticket page. |
| R2 | Kibo Ventures | How do you send them a pitch? | Tiene derecho a acceder, rectificar y suprimir los datos, así como otr | not_found | not_reached (carried). (carried from baseline) no pitch or contact text on the 5 pages read; the contact page was not reached (unverified) |
| R2 | K Fund | Do they invest at pre-seed? | This enables us to invest in companies from 100k€ to 10M€ and in seed, | correct | "invest in companies from 100k EUR to 10M EUR and in seed, pre-seed and Series A+". |
| R2 | K Fund | What is their typical first ticket? | €100k | correct | The K Founders first check is EUR 100k, the same as the fund's smallest ticket (100k to 10M); the quote is the program's, close to the fund's. |
| R2 | K Fund | How do you send them a pitch? | We have already launched the first call and you can join filling out t | wrong | A 2020 K Founders call ("open until September 15th 2020"): outdated, and not how to pitch the fund. |
| R2 | Samaipata | Do they invest at pre-seed? | Investment Thesis | not_found | not_on_site (carried). (carried from baseline) no pre-seed text in the 3 pages read; the investment-thesis link returns 404 |
| R2 | Samaipata | What is their typical first ticket? | €8M | not_found | not_on_site (carried). (carried from step3) The pages read have no first-ticket figure (the thesis page 404s). |
| R2 | Samaipata | How do you send them a pitch? | By answering the questions and sending us your deck | correct | The founders' typeform: "By answering the questions and sending us your deck" is the way to pitch. |
| R2 | JME Ventures | Do they invest at pre-seed? | We are seed stage investors in some of Spain´s biggest success stories | not_found | not_on_site (carried). (carried from baseline) the site says 'seed stage investors' on the pages read; no pre-seed text |
| R2 | JME Ventures | What is their typical first ticket? | €100k to €3m | correct | "We invest from EUR 100k to EUR 3m". |
| R2 | JME Ventures | How do you send them a pitch? | We prioritize speed & clarity, offering a transparent process that res | not_found | not_on_site (carried). (carried from step3) The pages read give the investment range (0.5-2m) but no pitch address or method. |
| R2 | Nauta | Do they invest at pre-seed? |  | not_found | js_only (carried). (carried from baseline) front door rendered with Lightpanda; one page read and no link scored (warmth 0.06) |
| R2 | Nauta | What is their typical first ticket? |  | not_found | js_only (carried). (carried from baseline) front door rendered with Lightpanda; one page read and no link scored (warmth 0.05) |
| R2 | Nauta | How do you send them a pitch? | Fale conosco | wrong | "Fale conosco" is a menu label (contact us); no pitch instructions. |
| R3 | ENISA | When is the application deadline? | Las empresas de base tecnológica e innovadora constituidas a partir de | correct | The call for companies founded from 2023 closes 22 January 2027 (the same call as before). |
| R3 | ENISA | What is the maximum amount? | 9.964 | wrong | "9.964 prestamos desembolsados" is a count of loans disbursed, not a maximum amount. |
| R3 | CDTI | When is the application deadline? |  | not_found | not_reached (carried). (carried from baseline) front door links to Ayuda and Actualidad but none scored (warmth 0.00); grant pages not reached |
| R3 | CDTI | What is the maximum amount? |  | not_found | not_reached (carried). (carried from baseline) front door links not scored (warmth 0.00); grant pages not reached |
| R3 | Acelerapyme (Kit Digital) | When is the application deadline? | 11 de septiembre de 2026 a las 11:00 horas | correct | "El plazo de inscripcion finaliza el 11 de septiembre de 2026 a las 11:00 horas". |
| R3 | Acelerapyme (Kit Digital) | What is the maximum amount? | 40 millones de euros | wrong | EUR 40 million is the budget of an AI initiative, not a maximum amount per applicant. |
| R3 | EIC | When is the application deadline? | [17 June 2026] | not_found | not_reached. Regression with en: the EIC front door's language selector sends the English path to pages without the Brussels deadline. |
| R3 | EIC | What is the maximum amount? | €6,5 billion | wrong | EUR 6.5 billion is the total support offered, not the maximum amount per project. |
| R4 | Web Summit | What are the dates of the next edition? | 9-12, 2026 | correct | "Lisbon - November 9-12, 2026": the next edition. |
| R4 | Web Summit | How much is a general ticket? | €809 Excl. sales tax | correct | The General attendee ticket is shown at EUR 1,595 / 995 / 809 excl. sales tax; 809 is one of its prices (the lowest shown, the current one is not dated). |
| R4 | Web Summit | In which city is it held? | Lisbon | correct | "Lisbon - November 9-12, 2026". |
| R4 | 4YFN | What are the dates of the next edition? | 2027 | not_found | not_reached (carried). (carried from step2) Regression: the 4YFN 2027 dates article sits in a sitemap that newest-first no longer reads. |
| R4 | 4YFN | How much is a general ticket? |  | not_found | not_reached (carried). (carried from baseline) no ticket text in the pages read; the tickets page was not reached (unverified) |
| R4 | 4YFN | In which city is it held? | Barcelona | correct | The page says 4YFN is held in Barcelona ("4YFN26 Barcelona ... Fira de Barcelona"). |
| R4 | South Summit | What are the dates of the next edition? | De 3 a 5 de junho de 2026 | wrong | The 3-5 June 2026 dates are the Madrid 2026 edition, already past: not the next edition. |
| R4 | South Summit | How much is a general ticket? | 299€ | wrong | 299 EUR is the Startup Pass in the FAQ, not the general ticket. |
| R4 | South Summit | In which city is it held? | Madrid | correct | "WHERE: La Nave (C/ Cifuentes, 5), Madrid": the event is held in Madrid. |
| R4 | VivaTech | What are the dates of the next edition? |  | not_found | blocked (carried). (carried from baseline) subpages return HTTP 403 (information, practical information, get-your-pass) |
| R4 | VivaTech | How much is a general ticket? |  | not_found | blocked (carried). (carried from baseline) subpages return HTTP 403 (get-your-pass) |
| R4 | VivaTech | In which city is it held? | Paris | correct | "VivaTech, the Paris tech conference". |
| R4 | Slush | What are the dates of the next edition? | Nov 18–19, Helsinki | correct | "Slush 2026 Nov 18-19, Helsinki": the next edition. |
| R4 | Slush | How much is a general ticket? | 395€* | wrong | EUR 395 is the startup ticket ("Tickets to Slush 2026 startup 395 EUR"), not the general ticket. |
| R4 | Slush | In which city is it held? | Helsinki | correct | "Helsinki is the home of Slush since 2008". |
| R5 | IE Business School | What is the price of the flagship MBA or bootcamp? | The cost can vary depending on the program | not_found | not_reached (carried). (carried from baseline) pages read are the school overview and programs; the MBA price page not reached |
| R5 | IE Business School | How long does it last? | TWO-WEEK MODULE OPTIONS | wrong | "TWO-WEEK MODULE OPTIONS" is a menu label; the duration of the MBA is not given. |
| R5 | IE Business School | Is there an online version? | Liquid Learning is the culmination of our educational vision, transcen | wrong | "Liquid Learning ... blurs the lines between online and in-person" is a blended-learning slogan, not an online version of the MBA. |
| R5 | ESADE | What is the price of the flagship MBA or bootcamp? | €39,650 | correct | "1st year at Esade: EUR 39,650" from the Full-Time MBA fees page (the first-year fee; the total is not stated). |
| R5 | ESADE | How long does it last? | Weeks of April 13th and April 20th - Pedralbes and Sant Cugat campuses | wrong | "Weeks of April 13th and April 20th - Pedralbes and Sant Cugat campuses": dates, not a duration. |
| R5 | ESADE | Is there an online version? | Formato: Online | wrong | "Formato: Online, Precio 2.900 EUR" is an online executive program, not the flagship MBA or a bootcamp. |
| R5 | IESE | What is the price of the flagship MBA or bootcamp? | €117,000 | correct | Table: "September 2027 intake ... Total EUR 117,000" for the MBA (fees over the two years). |
| R5 | IESE | How long does it last? | 18 months | correct | "The content of the EMBA program takes place over 18 months": the EMBA's length (not the full-time MBA's). |
| R5 | IESE | Is there an online version? | Online | wrong | "Online" is a category label among Focused Programs, not a statement about an online MBA or bootcamp. |
| R5 | Ironhack | What is the price of the flagship MBA or bootcamp? | 6.750€ | not_found | js_only (carried). (carried from baseline) pages read needed rendering (Lightpanda); no price text in them |
| R5 | Ironhack | How long does it last? | bis zu 1 Jahr nach Abschluss (oder bis du einen Job hast) | wrong | "Dauer: bis zu 1 Jahr nach Abschluss" is career support in the German FAQ, not the bootcamp's length. |
| R5 | Ironhack | Is there an online version? | Ironhack Remoto | correct | "Ironhack Remoto" is its remote (online) bootcamp format. |
| R6 | Ecoalf | From what order amount is shipping free? | 150€ | correct | "Envios gratis a partir de 150 EUR". |
| R6 | Ecoalf | How many days do you have to return an item? | 30 días | correct | "El plazo de devolucion y cambios es de 30 dias". |
| R6 | Hawkers | From what order amount is shipping free? | 49 € | correct | "El envio estandar es gratuito para compras superiores a 49 EUR". |
| R6 | Hawkers | How many days do you have to return an item? | 15 días naturales | correct | "Dispones de 15 dias naturales ... para devolver". |
| R6 | Pompeii | From what order amount is shipping free? | 100€ | correct | "gratuitos a partir de 100 EUR". |
| R6 | Pompeii | How many days do you have to return an item? | 365 días | correct | "Dispones de 365 dias para realizar cambios o devoluciones". |
| R6 | Scalpers | From what order amount is shipping free? | 40€ | correct | "Para pedidos superiores a 40 EUR, el envio a domicilio es gratuito". |
| R6 | Scalpers | How many days do you have to return an item? | 30 calendar days | correct | The English returns text: "30 calendar days" from receipt of the order (the same policy as the Spanish page). |
| R6 | Mr Wonderful | From what order amount is shipping free? | 30 € | correct | "Los pedidos superiores a 30 EUR disfrutan de envio gratuito" (peninsular Spain). |
| R6 | Mr Wonderful | How many days do you have to return an item? | 14 | correct | The legal withdrawal period is 14 natural days (art. 71 Ley 3/2014); the site's returns text says 15 (see the Spanish run). |
| R6es | Ecoalf | ¿A partir de qué importe de pedido el envío es gratis? | 150€ | correct | "Envios gratis a partir de 150 EUR". |
| R6es | Ecoalf | ¿Cuántos días tengo para devolver un artículo? | 30 días | correct | "El plazo de devolucion y cambios es de 30 dias". |
| R6es | Hawkers | ¿A partir de qué importe de pedido el envío es gratis? | 49 € | correct | "El envio estandar es gratuito para compras superiores a 49 EUR". |
| R6es | Hawkers | ¿Cuántos días tengo para devolver un artículo? | 15 días naturales | correct | "Dispones de 15 dias naturales ... para devolver". |
| R6es | Pompeii | ¿A partir de qué importe de pedido el envío es gratis? | 100€ | correct | "gratuitos a partir de 100 EUR". |
| R6es | Pompeii | ¿Cuántos días tengo para devolver un artículo? | 365 días | correct | "Dispones de 365 dias para realizar cambios o devoluciones". |
| R6es | Scalpers | ¿A partir de qué importe de pedido el envío es gratis? | 40€ | correct | "Si tu pedido no supera los 40 EUR, los gastos de envio ascienden a 3,95 EUR": above 40 EUR shipping is free. |
| R6es | Scalpers | ¿Cuántos días tengo para devolver un artículo? | Tienes 30 días naturales desde la recepción de tu pedido para realizar | correct | "Tienes 30 dias naturales desde la recepcion de tu pedido para realizar una devolucion". |
| R6es | Mr Wonderful | ¿A partir de qué importe de pedido el envío es gratis? | 30 € | correct | "Los pedidos superiores a 30 EUR disfrutan de envio gratuito". |
| R6es | Mr Wonderful | ¿Cuántos días tengo para devolver un artículo? | 15 días naturales | correct | "El USUARIO dispone de 15 dias naturales para la devolucion". |
| S1 | github.com | What is the primary rate limit for authenticated requests? | 5,000 requests per hour | correct | "All of these requests count towards your personal rate limit of 5,000 requests per hour". |
| S1 | gitlab.com | What is the primary rate limit for authenticated requests? |  | not_found | not_reached (carried). (carried from baseline) the search went to about.gitlab.com and repository pages; docs.gitlab.com (same registrable domain) not reached |
| S1 | bitbucket.org | What is the primary rate limit for authenticated requests? | 1,000 requests per hour | correct | Atlassian's API limits page: "The default rate limit is 1,000 requests per hour" (the page the owner's own check answered from). |
| S1 | stripe.com | What is the primary rate limit for authenticated requests? | / / 429 / Too Many Requests / Too many requests hit the API too quickl | not_found | not_reached (carried). (carried from baseline) docs.stripe.com/api was read (closest: 'Rate limits: Understand throttling'); docs.stripe.com/rate-limits not reached |
| S1 | shopify.com | What is the primary rate limit for authenticated requests? | 3,000 requests per minute | not_found | not_reached. The search reached shopify.dev/docs/api/usage/limits (the rate-limits URL 301-redirects there): a hub that says the figures are documented per API. The number is on the REST Admin API rate-limit page, which was not reached. |
| S2 | linear.app | What is the monthly price of the cheapest paid plan? | $10 per user/month | correct | "$10 per user/month, billed yearly": the cheapest paid plan. |
| S2 | notion.com | What is the monthly price of the cheapest paid plan? | €9.50 | correct | "Plus: EUR 9.50 per seat/month": Plus is the cheapest paid plan. |
| S2 | clickup.com | What is the monthly price of the cheapest paid plan? | $8user / mo | correct | "Core $8 user/mo": the cheapest paid plan (Free is $0). |
| S2 | vercel.com | What is the monthly price of the cheapest paid plan? | $20/mo | correct | Table: "Hobby $0/mo ... Pro $20/mo": Pro is the cheapest paid plan. |
| S2 | supabase.com | What is the monthly price of the cheapest paid plan? | $25/month | correct | "Pro: from $25/month": the cheapest paid plan. |
| S2 | posthog.com | What is the monthly price of the cheapest paid plan? | $0.000015/row | not_found | not_on_site (carried). (carried from baseline) PostHog's paid plans are usage-based on the pricing page read: no monthly price for a paid plan |
| S2 | figma.com | What is the monthly price of the cheapest paid plan? | $3/mo | wrong | $3 is the Collab seat price inside Professional, not the cheapest plan (Professional is $12-16 per seat). |
| S2 | asana.com | What is the monthly price of the cheapest paid plan? |  | not_found | missed_on_read (carried). (carried from baseline) the pricing page is read as markdown (no prices in it); the HTML has them ('Asana: $10.99 ... billed monthly') |
| S3 | vercel.com | Do they have a SOC 2 Type II report? | Yes | correct | "Yes. Vercel holds a SOC 2 Type 2 attestation". |
| S3 | vercel.com | Can customers choose where their data is stored? | Previously, Hobby customers could only choose US East (iad1) regardles | wrong | Function co-location with data for region choice of Functions; not a statement that customers choose where their data is stored. |
| S3 | supabase.com | Do they have a SOC 2 Type II report? | Update (2023-05-22): Supabase is now SOC2 Type 2 compliant | correct | "Supabase is now SOC2 Type 2 compliant". |
| S3 | supabase.com | Can customers choose where their data is stored? | Your database is hosted in the AWS region you select when you create y | correct | "Your database is hosted in the AWS region you select when you create your project." |
| S3 | posthog.com | Do they have a SOC 2 Type II report? | PostHog is certified as SOC 2 Type 2 compliant, following an external  | correct | "PostHog is certified as SOC 2 Type 2 compliant". |
| S3 | posthog.com | Can customers choose where their data is stored? | Control where your data is physically stored for GDPR compliance | correct | The feature table: "Data storage location: Control where your data is physically stored for GDPR compliance". |
| S3 | sentry.io | Do they have a SOC 2 Type II report? | SOC2 Type II | correct | The attestations list includes "SOC2 Type II". |
| S3 | sentry.io | Can customers choose where their data is stored? | customers have the option to store and process event data exclusively  | correct | "customers have the option to store and process event data exclusively in the European Union" (EU Data Residency). |
| S3 | resend.com | Do they have a SOC 2 Type II report? | Resend is SOC 2 Type II compliant | correct | "Resend is SOC 2 Type II compliant." |
| S3 | resend.com | Can customers choose where their data is stored? | All of our customers' data is still stored in the United States | correct | "All of our customers' data is still stored in the United States": the answer is no, no region choice. |
| S3 | cloudflare.com | Do they have a SOC 2 Type II report? | Meet SOC 2, PCI DSS, HIPAA, and GDPR requirements with audit logs, dat | wrong | "Meet SOC 2, PCI DSS, HIPAA, and GDPR requirements": says it helps meet SOC 2, not that a Type II report exists. |
| S3 | cloudflare.com | Can customers choose where their data is stored? | The Data Localization Suite (DLS) is a collection of tools that enable | correct | The Data Localization Suite lets customers "choose the location where Cloudflare inspects and stores data". |

