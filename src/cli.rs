//! The command line: the URL and every flag, as clap parses them and `--help` shows them.

use clap::Parser;

use crate::answer::PRECISE_THRESHOLD;

/// The bar a result needs when `--threshold` isn't given.
const DEFAULT_THRESHOLD: f64 = 0.5;

/// curl, but it reads the page for you. Jev picks what matters; Clef looks at
/// the images. Everything printed is literally in the page.
#[derive(Parser)]
#[command(version)]
pub(crate) struct Args {
    /// The page to read (`-` with --image, --vision or --find: the images stdin lists, one address per line), `init` to set up your API keys, `login`, `logout` or `status` for jurl cloud, `update` to
    /// install the latest jurl, or `mcp` to serve jurl's tools to an AI agent (MCP over stdio)
    pub(crate) url: String,
    /// Keep what helps answer this question instead of a general summary
    #[arg(short = 'q', long)]
    pub(crate) ask: Option<String>,
    /// Print the page's content images (one URL per line, best first)
    #[arg(short, long)]
    pub(crate) image: bool,
    /// With --image: let Clef look at the pixels (slower)
    #[arg(long)]
    pub(crate) vision: bool,
    /// Find the images that show this ("a cathedral"): Clef looks at every candidate
    #[arg(short, long, value_name = "WHAT")]
    pub(crate) find: Option<String>,
    /// Print the links worth following (one URL per line, best first)
    #[arg(short, long)]
    pub(crate) links: bool,
    /// Only code blocks: examples, commands, snippets
    #[arg(short, long)]
    pub(crate) code: bool,
    /// With -q: print just the answer, in the page's own words, then the block it's in and a link to it.
    /// Exits 1 when no part of the page is exactly the answer (2 on errors)
    #[arg(short, long)]
    pub(crate) precise: bool,
    /// With -q: also the best content image of the page the answer comes from, in the same read (no page is loaded
    /// again), judged as content, not by the question. `--with-image=WHAT` has Clef find the image that shows WHAT there
    /// instead, as --find does. Only that page's images are judged, never those of the other pages a --follow reads.
    /// No answer, no image
    #[arg(long, value_name = "WHAT", num_args = 0..=1, require_equals = true)]
    pub(crate) with_image: Option<Option<String>>,
    /// With -q: also the link on the answer's page that WHAT names (`--with-link="buy tickets"`), in the same read (no
    /// page is loaded again): one of the page's own links, scored as --links -q scores them, never one made up. Only
    /// that page's links are scored, never those of the other pages a --follow reads. No answer, no link
    #[arg(long, value_name = "WHAT", require_equals = true)]
    pub(crate) with_link: Option<String>,
    /// With --precise -q: also the line of the answer's page that a second reading points at, under `line` (its text, its
    /// share of the choice and how sure Jev is the page answers at all): one choice over the page's numbered lines, in the
    /// same read. A caller can check the answer against it. No answer, no line
    #[arg(long)]
    pub(crate) with_line: bool,
    /// With -q: when the page doesn't answer, follow its links within the same site, most promising first,
    /// reading up to this many pages in all [default: 5]. A search from a public address reads only public addresses
    #[arg(long, value_name = "PAGES", num_args = 0..=1, default_missing_value = "5")]
    pub(crate) follow: Option<usize>,
    /// Read up to this many pages of a list (50 at most): the page's own next-page links (`rel="next"`, a `?page=2` one
    /// page on, or an item offset one page on, as `?offset=24` from a page of 24), one after another, judged as one
    /// list. A page that links none but whose own data names its next batch (`next`, `next_url`, `links.next`…, on its site) is
    /// followed there, as long as each batch holds the list's things. With --recipe, a page that links none is followed to its
    /// page number one on (`?page=2` added when it has none), until a page gives no new row. The default block count is multiplied by the pages read; -a keeps all. Not
    /// with --follow, --links, --image or --vision
    #[arg(long, value_name = "N", default_value_t = 1)]
    pub(crate) pages: usize,
    /// Run the page's JavaScript with Lightpanda first (automatic when a page has scripts but no text, or unfilled template placeholders)
    #[arg(short, long)]
    pub(crate) render: bool,
    /// Don't ask a page that answers 202, 403 or 503, or resets its stream, again with a browser's TLS fingerprint (JURL_NO_BROWSER_RETRY does the same)
    #[arg(long)]
    pub(crate) no_browser_retry: bool,
    /// What the run may read, set from the start URL once it is known (see `reach.rs`). Not a flag.
    #[arg(skip)]
    pub(crate) reach: crate::reach::Reach,
    /// Whether `JURL_PUBLIC_ONLY=1` is set (see `reach.rs`): it refuses an environment proxy and rendering as well. Not a flag.
    #[arg(skip)]
    pub(crate) public_only: bool,
    /// Whether `JURL_RENDER_SANDBOXED=1` is set (see `reach.rs`): a public run may render under `JURL_PUBLIC_ONLY`. Not a flag.
    #[arg(skip)]
    pub(crate) render_sandboxed: bool,
    /// The cookies this run has been sent: kept in memory for this run alone, and never read from or written to a file
    /// (see `fetch.rs`). Not a flag.
    #[arg(skip)]
    pub(crate) cookies: reqwest::cookie::Jar,
    /// The stealth sidecar this run is set up with (see `stealth.rs`), when `JURL_STEALTH_URL` and its token are both set. Its
    /// memo and its budget are this run's own. Not a flag.
    #[arg(skip)]
    pub(crate) stealth: Option<crate::stealth::Sidecar>,
    /// The fallback proxy this run is set up with (see `fallback.rs`), when `JURL_FALLBACK_PROXY` names one. The hosts it puts on
    /// the proxy are this run's own. Not a flag.
    #[arg(skip)]
    pub(crate) fallback: crate::fallback::Fallback,
    /// The country this run reads as, from `JURL_COUNTRY` (see `country.rs`): the pages of its region come first, and a start page
    /// the site sends to another country is read again through the fallback proxy. Not a flag.
    #[arg(skip)]
    pub(crate) country: Option<String>,
    /// The Jev input tokens a `--follow` search may spend before it stops opening pages, from `JURL_JEV_MAX_TOKENS` (see
    /// `follow::jev_budget`). None: no cap. Not a flag.
    #[arg(skip)]
    pub(crate) jev_budget: Option<u64>,
    /// The pages this run has loaded, for `--with-pages` (see `output::PagesLog`). Its own to each run, as the cookies are.
    /// Not a flag.
    #[arg(skip)]
    pub(crate) pages_log: crate::output::PagesLog,
    /// Where a `--pages` read's list goes on after the pages it read (see `paging::ListNext`). Not a flag.
    #[arg(skip)]
    pub(crate) list_next: crate::paging::ListNext,
    /// Max results [default: 12 blocks, 5 with --ask, 8 code blocks, 20 links, all images]
    #[arg(short = 'n', long)]
    pub(crate) max: Option<usize>,
    /// No max: keep everything that passes the threshold
    #[arg(short, long)]
    pub(crate) all: bool,
    /// Minimum probability to keep a result [default: 0.5, or 0.4 for the --precise answer]
    #[arg(long)]
    pub(crate) threshold: Option<f64>,
    /// Print the result as JSON, with `usage` for what the run cost
    #[arg(long)]
    pub(crate) json: bool,
    /// With --json: the text of every page the run loaded, as `pages`, in load order (also the ones the answer doesn't come
    /// from), the things each page's JSON-LD lists and its photos. A miss or an error prints the JSON too, with `error`
    #[arg(long)]
    pub(crate) with_pages: bool,
    /// With --with-pages: also the lists of things each page's own data holds (its JSON-LD, its JSON scripts such as
    /// `__NEXT_DATA__`, and the objects its scripts assign), as `sources`: where each list is, how many it holds, and its
    /// first elements as `path: value` lines, to write a --recipe from
    #[arg(long, requires = "with_pages")]
    pub(crate) with_sources: bool,
    /// With --with-pages: take each page's `rows` from its own data with this recipe, with no model. JSON:
    /// `{"source": "json:__NEXT_DATA__", "path": ["props", "events"], "fields": {"title": ["name"], "date": ["dates", "start"]}}`.
    /// A step ending in `*` is the one key that starts with it. Each value is the page's own, as written
    #[arg(long, value_name = "JSON", requires = "with_pages", value_parser = crate::extract::sources::parse_recipe)]
    pub(crate) recipe: Option<crate::extract::sources::Recipe>,
    /// With --with-pages: load the pages and print them, with no model: no block is scored and nothing is answered, so the
    /// run is a miss whose JSON holds the pages, for a caller that reads them itself
    #[arg(long, requires = "with_pages")]
    pub(crate) pages_only: bool,
    /// Per-phase timings on stderr
    #[arg(short, long)]
    pub(crate) timing: bool,
}

impl Args {
    /// The probability a result needs to be kept: `--threshold`, or 0.5.
    pub(crate) fn threshold(&self) -> f64 {
        self.threshold.unwrap_or(DEFAULT_THRESHOLD)
    }

    /// The same bar for a mode: `precise` is `--precise`, whose answer has its own default (`PRECISE_THRESHOLD`).
    pub(crate) fn threshold_for(&self, precise: bool) -> f64 {
        if precise { self.threshold.unwrap_or(PRECISE_THRESHOLD) } else { self.threshold() }
    }

    pub(crate) fn limit(&self, default: usize) -> usize {
        if self.all { usize::MAX } else { self.max.unwrap_or(default) }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn every_flag_has_help_text() {
        for arg in Args::command().get_arguments() {
            assert!(arg.get_help().is_some(), "--{} has no help text", arg.get_id());
        }
    }

    #[test]
    fn a_flag_without_a_url_is_still_clap_s_usage_error() {
        // Only `jurl` alone starts the setup or the help (main.rs). A flag without a URL is still an error.
        for argv in [vec!["jurl"], vec!["jurl", "-q", "why?"], vec!["jurl", "--json"]] {
            let err = Args::try_parse_from(argv.clone()).err();
            assert_eq!(err.map(|e| e.kind()), Some(clap::error::ErrorKind::MissingRequiredArgument), "{argv:?}");
        }
        assert!(Args::try_parse_from(["jurl", "-q", "why?", "x.com"]).is_ok());
    }

    #[test]
    fn each_mode_has_its_default_bar_and_threshold_overrides_both() {
        let plain = Args::parse_from(["jurl", "x.com"]);
        assert_eq!(plain.threshold_for(false), 0.5);
        assert_eq!(plain.threshold_for(true), PRECISE_THRESHOLD);
        let given = Args::parse_from(["jurl", "--threshold", "0.7", "x.com"]);
        assert_eq!(given.threshold_for(false), 0.7);
        assert_eq!(given.threshold_for(true), 0.7);
    }
}
