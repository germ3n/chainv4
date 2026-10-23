use crate::Page;

const HEAD: &str = r####"<!DOCTYPE html>
<html lang="en">
<head>
<script>
try {
  if (localStorage.getItem("wiki-theme") === "light") {
    document.documentElement.setAttribute("data-theme", "light");
  }
} catch (err) {}
</script>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>chainv4 Wiki</title>
<style>
/*__WIKI_FONTS__*/
:root {
  color-scheme: dark;
  --paper: #0c0d10;
  --nav: #0f1013;
  --card: #14161a;
  --card-2: #181a1f;
  --code-bg: #111317;
  --code-ink: #e6e2da;
  --ink: #ece8e1;
  --soft: #c4c0b7;
  --muted: #86837c;
  --heading: #a7a39a;
  --line: #24262b;
  --row: #1b1d22;
  --accent: #d6aa62;
  --accent-2: #ebc886;
  --accent-soft: rgba(214, 170, 98, 0.12);
  --glow: rgba(214, 170, 98, 0.07);
  --hover: rgba(255, 255, 255, 0.045);
  --type: #8fb9d8;
  --kw: #d49bb0;
  --str: #a3c99c;
  --num: #e3ae72;
  --com: #6e6c66;
  --fn: #e0cf8e;
  --dot-client: #e0a458;
  --dot-server: #6aa4e8;
  --dot-menu: #b58be0;
  --warn-bg: rgba(224, 164, 88, 0.10);
  --warn-ink: #e8b872;
  --bad-bg: rgba(226, 98, 98, 0.10);
  --bad-ink: #ec8d8d;
  --shadow: rgba(0, 0, 0, 0.45);
  --sans: "Manrope", "Segoe UI", system-ui, sans-serif;
  --serif: "Newsreader", Georgia, "Times New Roman", serif;
  --mono: "JetBrains Mono", ui-monospace, Menlo, Consolas, monospace;
  /* Every section starts its description at the same offset: the lead column plus the gutter. */
  --lead: 326px;
  --gutter: 24px;
}
html[data-theme="light"] {
  color-scheme: light;
  --paper: #f8f5ef;
  --nav: #f3efe7;
  --card: #fffdf9;
  --card-2: #fbf8f2;
  --code-bg: #f1ece1;
  --code-ink: #26231e;
  --ink: #1d1b17;
  --soft: #46423a;
  --muted: #7b766b;
  --heading: #6a655b;
  --line: #e2dccf;
  --row: #ebe5d8;
  --accent: #98681b;
  --accent-2: #b8862e;
  --accent-soft: rgba(152, 104, 27, 0.10);
  --glow: rgba(184, 134, 46, 0.09);
  --hover: rgba(40, 30, 10, 0.05);
  --type: #2c6a97;
  --kw: #a2456c;
  --str: #3f7a3a;
  --num: #a8631a;
  --com: #9a958a;
  --fn: #8a6a12;
  --dot-client: #c0771c;
  --dot-server: #2f74d0;
  --dot-menu: #8554c0;
  --warn-bg: rgba(192, 119, 28, 0.10);
  --warn-ink: #8a5712;
  --bad-bg: rgba(190, 70, 70, 0.09);
  --bad-ink: #a23838;
  --shadow: rgba(60, 40, 10, 0.10);
}
* { box-sizing: border-box; }
html, body { margin: 0; padding: 0; }
html { scroll-behavior: smooth; }
body {
  font-family: var(--sans);
  font-size: 15px;
  line-height: 1.6;
  color: var(--ink);
  background: var(--paper);
  font-synthesis: none;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
  text-rendering: optimizeLegibility;
}
button, input { font: inherit; }
::selection { background: var(--accent-soft); color: var(--ink); }
:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
#app { display: flex; align-items: flex-start; min-height: 100vh; }

aside {
  width: 304px;
  flex: none;
  position: sticky;
  top: 0;
  height: 100vh;
  overflow: auto;
  background: var(--nav);
  border-right: 1px solid var(--line);
  color: var(--soft);
  padding: 30px 18px 40px 20px;
  scrollbar-width: thin;
  scrollbar-color: var(--line) transparent;
}
aside::-webkit-scrollbar { width: 8px; }
aside::-webkit-scrollbar-thumb { background: var(--line); border-radius: 99px; }
.brand-row {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 8px;
  padding: 0 6px 22px;
}
.brand { padding: 0; display: block; color: inherit; text-decoration: none; cursor: pointer; }
.brand:hover strong::before { transform: rotate(45deg) scale(1.25); }
.brand strong::before { transition: transform 0.15s ease; }
.brand:focus-visible { outline: 2px solid var(--accent); outline-offset: 4px; border-radius: 4px; }
.brand strong {
  display: flex;
  align-items: center;
  gap: 11px;
  color: var(--ink);
  font-family: var(--serif);
  font-weight: 400;
  font-size: 28px;
  line-height: 1;
  letter-spacing: -0.01em;
}
.brand strong::before {
  content: "";
  width: 8px;
  height: 8px;
  background: var(--accent);
  transform: rotate(45deg);
  flex: none;
}
.brand span {
  display: block;
  margin: 9px 0 0 19px;
  color: var(--muted);
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.24em;
  text-transform: uppercase;
}
#theme {
  border: 1px solid var(--line);
  background: transparent;
  color: var(--muted);
  border-radius: 999px;
  padding: 5px 11px;
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.14em;
  text-transform: uppercase;
  cursor: pointer;
  transition: color 0.15s, border-color 0.15s;
}
#theme:hover { color: var(--accent); border-color: var(--accent); }
.search-wrap { position: relative; margin: 0 4px; }
#search {
  width: 100%;
  border: 1px solid var(--line);
  background: var(--card);
  color: var(--ink);
  border-radius: 10px;
  padding: 10px 34px 10px 13px;
  font-size: 14px;
  outline: none;
  transition: border-color 0.15s, box-shadow 0.15s;
}
#search::placeholder { color: var(--muted); }
#search:focus { border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft); }
.search-wrap kbd {
  position: absolute;
  right: 10px;
  top: 50%;
  transform: translateY(-50%);
  font-family: var(--mono);
  font-size: 11px;
  color: var(--muted);
  border: 1px solid var(--line);
  border-radius: 5px;
  padding: 0 6px;
  line-height: 18px;
  pointer-events: none;
}
#search:focus ~ kbd, #search:not(:placeholder-shown) ~ kbd { display: none; }
.group { margin-top: 28px; }
.group h2 {
  display: flex;
  align-items: center;
  gap: 12px;
  margin: 0 6px 8px;
  font-size: 10px;
  letter-spacing: 0.22em;
  text-transform: uppercase;
  color: var(--muted);
  font-weight: 600;
}
.group h2::after { content: ""; flex: 1; height: 1px; background: var(--line); }
.nav-link {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  color: var(--soft);
  text-decoration: none;
  border-radius: 7px;
  padding: 5px 10px;
  font-size: 14px;
  transition: background 0.12s, color 0.12s;
}
.nav-link:hover { background: var(--hover); color: var(--ink); }
.nav-link.active { background: var(--accent-soft); color: var(--accent); font-weight: 600; }
.nav-link.active::before {
  content: "";
  position: absolute;
  left: 0;
  top: 7px;
  bottom: 7px;
  width: 2px;
  border-radius: 2px;
  background: var(--accent);
}
.branch:not(.open) > .kids { display: none; }
.kids {
  margin: 2px 0 8px 14px;
  padding-left: 8px;
  border-left: 1px solid var(--line);
}
.kids .nav-link { font-size: 13px; color: var(--muted); padding: 3px 10px; }
.kids .nav-link:hover { color: var(--ink); }
.kids .nav-link.active { color: var(--accent); }
.folder-name {
  display: block;
  margin: 10px 10px 2px;
  font-size: 9.5px;
  letter-spacing: 0.2em;
  text-transform: uppercase;
  color: var(--muted);
  font-weight: 600;
  opacity: 0.75;
}
h4.folder-title {
  margin: 20px 0 6px;
  font-size: 10.5px;
  font-weight: 600;
  letter-spacing: 0.16em;
  text-transform: uppercase;
  color: var(--muted);
}

main {
  flex: 1;
  min-width: 0;
  padding: 60px 72px 140px;
  background: radial-gradient(900px 420px at 18% -8%, var(--glow), transparent 70%) no-repeat;
}
article {
  max-width: 820px;
  animation: rise 0.35s cubic-bezier(0.2, 0.7, 0.2, 1) both;
}
article.home { max-width: none; }
@keyframes rise {
  from { opacity: 0; transform: translateY(8px); }
  to { opacity: 1; transform: none; }
}
@media (prefers-reduced-motion: reduce) {
  article { animation: none; }
  html { scroll-behavior: auto; }
}
.crumbs { margin: 0 0 30px; color: var(--muted); font-size: 12.5px; letter-spacing: 0.02em; }
.crumbs a { color: var(--muted); text-decoration: none; transition: color 0.12s; }
.crumbs a:hover { color: var(--accent); }
.crumbs span { margin: 0 3px; opacity: 0.55; }
.title-row {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 24px;
  flex-wrap: wrap;
}
.eyebrow {
  margin: 0 0 12px;
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.22em;
  text-transform: uppercase;
  color: var(--accent);
}
h1 {
  margin: 0;
  font-family: var(--serif);
  font-weight: 400;
  font-size: 48px;
  line-height: 1.06;
  letter-spacing: -0.025em;
  overflow-wrap: anywhere;
}
.realms { display: flex; gap: 6px; flex: none; padding-bottom: 8px; flex-wrap: wrap; }
.realm, .lang, .access {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  font-size: 10.5px;
  font-weight: 600;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: var(--muted);
  border: 1px solid var(--line);
  border-radius: 999px;
  padding: 3px 10px 3px 8px;
  flex: none;
  line-height: 1.5;
}
.realm::before {
  content: "";
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--dot);
  flex: none;
}
.realm.client { --dot: var(--dot-client); }
.realm.server { --dot: var(--dot-server); }
.realm.menu { --dot: var(--dot-menu); }
.access, .lang { padding-left: 10px; }
.lang { color: var(--soft); }
.access.internal { color: var(--accent); border-color: var(--accent); }
.nav-link .realm {
  font-size: 0;
  gap: 0;
  padding: 0;
  border: 0;
  width: 7px;
  height: 7px;
}
.nav-link .realm::before { width: 6px; height: 6px; opacity: 0.85; }
.badges { display: flex; gap: 5px; align-items: center; flex: none; }
.member-name .realm { font-size: 0; gap: 0; padding: 0; border: 0; }
pre.sig, pre.code {
  font-family: var(--mono);
  overflow-x: auto;
  scrollbar-width: thin;
  scrollbar-color: var(--line) transparent;
}
pre.sig {
  margin: 30px 0 0;
  padding: 18px 22px;
  background: var(--code-bg);
  color: var(--code-ink);
  border: 1px solid var(--line);
  border-left: 2px solid var(--accent);
  border-radius: 4px 12px 12px 4px;
  font-size: 14px;
  line-height: 1.65;
}
pre.sig .type, pre.sig a.type { color: var(--type); text-decoration: none; }
pre.sig a.type:hover { text-decoration: underline; text-underline-offset: 3px; }
.summary {
  margin: 28px 0 0;
  max-width: 64ch;
  font-family: var(--serif);
  font-size: 21px;
  line-height: 1.55;
  color: var(--soft);
}
.since { color: var(--muted); font-size: 13px; font-style: italic; font-family: var(--serif); }
h3 {
  display: flex;
  align-items: center;
  gap: 16px;
  margin: 52px 0 16px;
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.22em;
  text-transform: uppercase;
  color: var(--heading);
}
h3::after { content: ""; flex: 1; height: 1px; background: var(--line); }
article > p:not(.crumbs):not(.summary):not(.since):not(.banner):not(.eyebrow) {
  margin: 0;
  max-width: 68ch;
  color: var(--soft);
  line-height: 1.7;
}
table.args { width: 100%; border-collapse: collapse; }
table.args th {
  text-align: left;
  font-size: 10.5px;
  letter-spacing: 0.16em;
  text-transform: uppercase;
  color: var(--muted);
  font-weight: 600;
  padding: 0 16px 10px 0;
  border-bottom: 1px solid var(--line);
}
table.args td {
  padding: 13px 16px 13px 0;
  vertical-align: top;
  border-bottom: 1px solid var(--row);
  line-height: 1.55;
  color: var(--soft);
}
table.args tr:last-child td { border-bottom: 0; }
table.args { table-layout: fixed; }
table.args:not(.ret) th:nth-child(1), table.args:not(.ret) td:nth-child(1) { width: 160px; }
table.args:not(.ret) th:nth-child(2), table.args:not(.ret) td:nth-child(2) { width: calc(var(--lead) + var(--gutter) - 160px); }
table.ret th:first-child, table.ret td:first-child { width: calc(var(--lead) + var(--gutter)); }
table.args td .type { overflow-wrap: anywhere; }
table.args td.name {
  font-family: var(--mono);
  font-size: 13.5px;
  color: var(--ink);
  overflow-wrap: anywhere;
}
em.opt, em.opt {
  font-style: normal;
  font-family: var(--sans);
  font-size: 9.5px;
  font-weight: 600;
  letter-spacing: 0.14em;
  text-transform: uppercase;
  color: var(--muted);
  border: 1px solid var(--line);
  border-radius: 999px;
  padding: 1px 7px;
  margin-left: 8px;
}
a.type { color: var(--type); text-decoration: none; font-family: var(--mono); font-size: 13px; }
a.type:hover { text-decoration: underline; text-underline-offset: 3px; }
span.type { font-family: var(--mono); font-size: 13px; color: var(--type); }
pre.code {
  margin: 0;
  padding: 18px 22px;
  background: var(--code-bg);
  color: var(--code-ink);
  border: 1px solid var(--line);
  border-radius: 12px;
  line-height: 1.7;
  font-size: 13.5px;
}
pre.code .kw { color: var(--kw); }
pre.code .str { color: var(--str); }
pre.code .num { color: var(--num); }
pre.code .com { color: var(--com); font-style: italic; }
.banner {
  padding: 12px 16px;
  border-radius: 4px 10px 10px 4px;
  margin: 24px 0 0;
  font-size: 14px;
  border-left: 2px solid currentColor;
}
.banner.warn { background: var(--warn-bg); color: var(--warn-ink); }
.banner.bad { background: var(--bad-bg); color: var(--bad-ink); }
ul.members, ul.seealso { list-style: none; padding: 0; margin: 0; }
ul.members li {
  display: grid;
  grid-template-columns: var(--lead) 1fr;
  gap: 8px var(--gutter);
  padding: 11px 0;
  border-bottom: 1px solid var(--row);
  align-items: baseline;
}
ul.members li:last-child { border-bottom: 0; }
.member-name { display: flex; align-items: center; gap: 9px; flex-wrap: wrap; }
ul.members a {
  font-family: var(--mono);
  font-size: 13.5px;
  color: var(--ink);
  text-decoration: none;
  transition: color 0.12s;
}
ul.members li:hover a { color: var(--accent); }
ul.members li > span:last-child { color: var(--muted); font-size: 14px; }
ul.seealso li { padding: 4px 0; color: var(--muted); }
ul.seealso a {
  color: var(--accent);
  text-decoration: none;
  font-family: var(--mono);
  font-size: 13.5px;
}
ul.seealso a:hover { text-decoration: underline; text-underline-offset: 3px; }

.hero { padding: 24px 0 8px; }
.hero h1 { font-size: 76px; line-height: 1; letter-spacing: -0.035em; }
.hero h1 em { font-style: italic; color: var(--accent); }
.hero .lede {
  margin: 28px 0 0;
  max-width: 54ch;
  font-family: var(--serif);
  font-size: 22px;
  line-height: 1.55;
  color: var(--muted);
}
.hero .rule {
  width: 56px;
  height: 1px;
  margin-top: 36px;
  background: var(--accent);
}
.cards {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
  gap: 16px;
}
.card {
  display: block;
  text-decoration: none;
  color: inherit;
  background: var(--card);
  border: 1px solid var(--line);
  border-radius: 14px;
  padding: 20px 22px 22px;
  transition: transform 0.18s ease, border-color 0.18s ease, box-shadow 0.18s ease;
}
.card:hover {
  transform: translateY(-3px);
  border-color: var(--accent);
  box-shadow: 0 14px 34px -14px var(--shadow);
}
.card strong {
  display: block;
  font-family: var(--serif);
  font-weight: 400;
  font-size: 25px;
  line-height: 1.15;
  letter-spacing: -0.01em;
}
.card em {
  display: block;
  margin-top: 5px;
  color: var(--accent);
  font-style: normal;
  font-size: 10.5px;
  font-weight: 600;
  letter-spacing: 0.16em;
  text-transform: uppercase;
}
.card p { margin: 12px 0 0; color: var(--muted); font-size: 14px; line-height: 1.55; }
#menu {
  display: none;
  position: fixed;
  top: 14px;
  right: 14px;
  z-index: 3;
  border: 1px solid var(--line);
  background: var(--card);
  color: var(--ink);
  border-radius: 999px;
  padding: 8px 16px;
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.1em;
  text-transform: uppercase;
  cursor: pointer;
}
.realm-filter { display: flex; gap: 6px; margin: 10px 4px 0; flex-wrap: wrap; }
.chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  border: 1px solid var(--line);
  background: transparent;
  color: var(--muted);
  border-radius: 999px;
  padding: 4px 10px;
  font-family: inherit;
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.04em;
  cursor: pointer;
  transition: color 0.12s, border-color 0.12s, background 0.12s;
}
.chip i { width: 7px; height: 7px; border-radius: 50%; background: var(--dot); opacity: 0.35; transition: opacity 0.12s; }
.chip.client { --dot: var(--dot-client); }
.chip.server { --dot: var(--dot-server); }
.chip.menu { --dot: var(--dot-menu); }
.chip.on { color: var(--ink); border-color: var(--dot); background: var(--hover); }
.chip.on i { opacity: 1; }
.chip:hover { color: var(--ink); }
.results { display: flex; flex-direction: column; gap: 2px; margin-top: 14px; }
.results .none { color: var(--muted); font-size: 13px; padding: 10px; }
.result {
  display: block;
  color: var(--soft);
  text-decoration: none;
  border-radius: 7px;
  padding: 6px 10px;
  border: 1px solid transparent;
}
.result:hover { background: var(--hover); }
.result.selected { background: var(--accent-soft); border-color: var(--accent); color: var(--ink); }
.result .top { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.result .title { font-size: 14px; font-weight: 600; overflow-wrap: break-word; }
.result .kind { color: var(--muted); font-size: 10px; letter-spacing: 0.16em; text-transform: uppercase; margin-right: 8px; font-weight: 600; }
.result .sum {
  display: block;
  margin-top: 2px;
  color: var(--muted);
  font-size: 12px;
  line-height: 1.4;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
mark { background: var(--accent-soft); color: var(--accent); border-radius: 3px; padding: 0 1px; }
.nav-link.dep > span:first-child, .result.dep .title { text-decoration: line-through; text-decoration-thickness: 1px; opacity: 0.75; }
.tag {
  display: inline-block;
  margin-left: 8px;
  padding: 1px 7px;
  border: 1px solid var(--line);
  border-radius: 999px;
  color: var(--muted);
  font-family: var(--sans);
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.06em;
  vertical-align: middle;
}
.tag.warn { color: var(--warn-ink); border-color: var(--warn-ink); background: var(--warn-bg); }
.codewrap { position: relative; }
.codewrap .copy {
  position: absolute;
  top: 10px;
  right: 10px;
  border: 1px solid var(--line);
  background: var(--card);
  color: var(--muted);
  border-radius: 999px;
  padding: 3px 11px;
  font-family: var(--sans);
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.06em;
  cursor: pointer;
  opacity: 0;
  transition: opacity 0.12s, color 0.12s, border-color 0.12s;
}
.codewrap:hover .copy, .codewrap .copy:focus-visible { opacity: 1; }
.codewrap .copy:hover { color: var(--accent); border-color: var(--accent); }
.codewrap .copy.done { opacity: 1; color: var(--accent); border-color: var(--accent); }
@media (hover: none) { .codewrap .copy { opacity: 1; } }
pre.code .fn { color: var(--fn); }
pre.code .ty { color: var(--type); }
pre.sig .kw { color: var(--kw); }
.lang-switch {
  display: flex;
  gap: 4px;
  margin: 0 4px 14px;
  padding: 3px;
  border: 1px solid var(--line);
  border-radius: 999px;
  background: var(--card);
}
.lang-tab {
  flex: 1;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 5px 10px;
  border-radius: 999px;
  color: var(--muted);
  text-decoration: none;
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.06em;
  transition: color 0.12s, background 0.12s;
}
.lang-tab em { font-style: normal; font-family: var(--mono); font-size: 10.5px; font-weight: 400; opacity: 0.7; }
.lang-tab:hover { color: var(--ink); }
.lang-tab.on { background: var(--accent-soft); color: var(--accent); }
.realm-filter:empty { display: none; }
.results .none a { color: var(--accent); text-decoration: none; }
.results .none a:hover { text-decoration: underline; text-underline-offset: 3px; }
.empty { margin-top: 44px; max-width: 64ch; color: var(--muted); font-size: 15px; line-height: 1.7; }
.empty code { font-family: var(--mono); font-size: 13px; color: var(--ink); }
ul.seealso li { display: grid; grid-template-columns: var(--lead) 1fr; gap: 8px var(--gutter); align-items: baseline; }
ul.seealso li > * { min-width: 0; }
ul.seealso a { overflow-wrap: anywhere; }
ul.seealso li span { color: var(--muted); }
@media (max-width: 860px) { ul.seealso li { grid-template-columns: 1fr; gap: 2px; } }
@media (max-width: 1100px) {
  main { padding: 52px 40px 100px; }
  h1 { font-size: 40px; }
  .hero h1 { font-size: 60px; }
}
@media (max-width: 860px) {
  #menu { display: block; }
  aside {
    position: fixed;
    z-index: 2;
    transform: translateX(-105%);
    transition: transform 0.2s ease;
    box-shadow: 12px 0 40px var(--shadow);
  }
  aside.open { transform: none; }
  main { padding: 72px 20px 64px; }
  ul.members li { grid-template-columns: 1fr; gap: 3px; }
  table.args { table-layout: auto; }
  table.args:not(.ret) th:nth-child(1), table.args:not(.ret) td:nth-child(1), table.args:not(.ret) th:nth-child(2), table.args:not(.ret) td:nth-child(2), table.ret th:first-child, table.ret td:first-child { width: auto; }
  h1 { font-size: 32px; }
  .hero h1 { font-size: 44px; }
  .hero .lede { font-size: 19px; }
  .summary { font-size: 19px; }
  table.args td.name { white-space: normal; }
}
</style>
</head>
<body>
<div id="app">
<aside id="sidebar">
  <div class="brand-row"><a class="brand" href="#" title="Back to the homepage"><strong>Wiki</strong><span id="brand-sub">Rust API</span></a><button id="theme" type="button" aria-pressed="true">Light</button></div>
  <div class="lang-switch" id="lang-switch" role="group" aria-label="Language"></div>
  <div class="search-wrap"><input id="search" type="search" placeholder="Search the reference" autocomplete="off" aria-label="Search"><kbd>/</kbd></div>
  <div class="realm-filter" id="realm-filter" role="group" aria-label="Filter by realm"></div>
  <nav id="nav"></nav>
</aside>
<main id="main"></main>
</div>
<button id="menu" type="button">Menu</button>
<script id="wiki-data" type="application/json">"####;

const TAIL: &str = r####"</script>
<script>
(function () {
  var data = JSON.parse(document.getElementById("wiki-data").textContent);
  var byKey = {};
  var search = document.getElementById("search");
  var nav = document.getElementById("nav");
  var main = document.getElementById("main");
  var menu = document.getElementById("menu");
  var themeButton = document.getElementById("theme");
  var aside = document.getElementById("sidebar");
  var langSwitch = document.getElementById("lang-switch");
  var brandSub = document.getElementById("brand-sub");
  var idx;

  var TITLE = "chainv4 Wiki";
  var LANGS = ["rust", "lua", "wasm"];
  var LANG_LABELS = { rust: "Rust", lua: "Lua", wasm: "WASM" };
  var REALM_NAMES = ["client", "server", "menu"];
  var CONTAINERS = { library: true, class: true, module: true, struct: true, enum: true, trait: true };
  /* Top-level pages without a category land in a section named after their kind. These come
     after every category, in this order. */
  var SECTION_OF = {
    rust: { module: "Modules", struct: "Types", enum: "Types", trait: "Types", type: "Types", function: "Functions", constant: "Constants" },
    lua: { class: "Classes", library: "Libraries", function: "Functions" },
    wasm: { library: "Imports", function: "Functions" }
  };
  var SECTION_ORDER = {
    rust: ["Modules", "Types", "Functions", "Constants"],
    lua: ["Classes", "Libraries", "Functions"],
    wasm: ["Imports", "Functions"]
  };
  var MEMBER_SECTIONS = [
    ["variant", "Variants"],
    ["field", "Fields"],
    ["method", "Methods"],
    ["function", "Functions"],
    ["constant", "Constants"],
    ["type", "Types"],
    ["hook", "Hooks"]
  ];
  var LEDES = {
    rust: "Rust reference for chainv4. Types are the structs, enums and traits the chain is built from. Modules group free functions and constants.",
    lua: "Lua scripting reference. Classes are values you call with a colon. Libraries are tables of functions. Hooks are callbacks you put on an object.",
    wasm: "WebAssembly contract reference: the host functions a module imports, with pointers and lengths as i32s into its exported memory. Write contracts in Rust with chainv4-sdk, which wraps every one of them."
  };
  var EMPTY = {
    rust: "Nothing is documented in Rust yet. Put <code>#[document(summary = \"…\")]</code> on an item.",
    lua: "Nothing is documented in Lua yet. Write a <code>--[=[document … ]=]</code> block in a .lua file, or add <code>lua</code> to a <code>#[document]</code> on the Rust side of a binding.",
    wasm: "Nothing is documented for WebAssembly yet. Add <code>wasm</code> to a <code>#[document]</code> on the Rust side of a host function."
  };
  var PANICS = { rust: "Panics", lua: "Errors", wasm: "Traps" };

  var referrers = {};
  var realmState = loadRealms();
  var lang = loadLang();
  var selected = 0;
  var shownResults = [];

  for (idx = 0; idx < data.length; idx++) {
    byKey[keyOf(data[idx])] = data[idx];
  }

  for (idx = 0; idx < data.length; idx++) {
    var refs = data[idx].see_also || [];

    for (var refIdx = 0; refIdx < refs.length; refIdx++) {
      (referrers[refs[refIdx]] = referrers[refs[refIdx]] || []).push(keyOf(data[idx]));
    }
  }

  function keyOf(page) {
    return page.lang + "/" + page.id;
  }

  function href(page) {
    return "#" + page.lang + "/" + encodeURIComponent(page.id);
  }

  function count(name) {
    var total = 0;

    for (var itemIdx = 0; itemIdx < data.length; itemIdx++) {
      if (data[itemIdx].lang === name) {
        total++;
      }
    }

    return total;
  }

  /* The other reference with the most matches for a search that found nothing here. */
  function otherLang(q) {
    var best = null;
    var most = 0;

    LANGS.forEach(function (name) {
      var found = name === lang ? 0 : searchPages(q, name).length;

      if (found > most) {
        best = name;
        most = found;
      }
    });

    return best || (lang === "rust" ? "lua" : "rust");
  }

  /* The language picked last time, otherwise the first one with pages. A link to a page of the
     other language switches over when it opens. */
  function loadLang() {
    var stored = null;

    try {
      stored = localStorage.getItem("wiki-lang");
    } catch (err) {}

    if (LANG_LABELS[stored]) {
      return stored;
    }

    for (var langIdx = 0; langIdx < LANGS.length; langIdx++) {
      if (count(LANGS[langIdx])) {
        return LANGS[langIdx];
      }
    }

    return LANGS[0];
  }

  function setLang(next) {
    if (lang === next) {
      return;
    }

    lang = next;
    selected = 0;

    try {
      localStorage.setItem("wiki-lang", next);
    } catch (err) {}
  }

  function loadRealms() {
    var state = { client: true, server: true, menu: true };

    try {
      var raw = JSON.parse(localStorage.getItem("wiki-realms"));

      if (raw && typeof raw === "object") {
        state.client = raw.client !== false;
        state.server = raw.server !== false;
        state.menu = raw.menu !== false;
      }
    } catch (err) {}

    return state;
  }

  function saveRealms() {
    try {
      localStorage.setItem("wiki-realms", JSON.stringify(realmState));
    } catch (err) {}
  }

  function realmNames(realm) {
    var out = [];

    realm.split(",").forEach(function (name) {
      if (name === "shared") {
        out.push("client", "server");
      } else {
        out.push(name);
      }
    });

    return out;
  }

  /* Pages that do not name a realm run everywhere, so the filter never hides them. */
  function realmVisible(page) {
    if (!page.realm) {
      return true;
    }

    var names = realmNames(page.realm);

    for (var nameIdx = 0; nameIdx < names.length; nameIdx++) {
      if (realmState[names[nameIdx]]) {
        return true;
      }
    }

    return false;
  }

  function usesRealms() {
    for (var itemIdx = 0; itemIdx < data.length; itemIdx++) {
      if (data[itemIdx].lang === lang && data[itemIdx].realm) {
        return true;
      }
    }

    return false;
  }

  function esc(text) {
    return String(text).replace(/[&<>"']/g, function (ch) {
      if (ch === "&") return "&amp;";
      if (ch === "<") return "&lt;";
      if (ch === ">") return "&gt;";
      if (ch === '"') return "&quot;";
      return "&#39;";
    });
  }

  /* "#rust/Tx::hash" is a page, "#lua" is the Lua home and "#" the home of the current
     language. An id without a language is looked up in the current one first. */
  function route() {
    var raw = location.hash.replace(/^#/, "");

    try {
      raw = decodeURIComponent(raw);
    } catch (err) {}

    var cut = raw.indexOf("/");
    var head = cut === -1 ? raw : raw.slice(0, cut);

    if (LANG_LABELS[head]) {
      return { lang: head, id: cut === -1 ? "" : raw.slice(cut + 1) };
    }

    return { lang: lang, id: raw };
  }

  function lookup(here) {
    if (!here.id) {
      return null;
    }

    if (byKey[here.lang + "/" + here.id]) {
      return byKey[here.lang + "/" + here.id];
    }

    for (var langIdx = 0; langIdx < LANGS.length; langIdx++) {
      var found = byKey[LANGS[langIdx] + "/" + here.id];

      if (found) {
        return found;
      }
    }

    return null;
  }

  function currentPage() {
    return lookup(route());
  }

  function currentKey() {
    var page = currentPage();

    return page ? keyOf(page) : "";
  }

  /* Follows the address into the language it points at. */
  function sync() {
    var here = route();
    var page = lookup(here);
    setLang(page ? page.lang : here.lang);
  }

  function query() {
    return search.value.trim().toLowerCase();
  }

  function tokens(q) {
    return q.split(/\s+/).filter(Boolean);
  }

  function reEsc(text) {
    return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  }

  function mark(text, toks) {
    if (!toks.length) {
      return esc(text);
    }

    var re = new RegExp("(" + toks.map(reEsc).join("|") + ")", "gi");

    return text.split(re).map(function (part, partIdx) {
      return partIdx % 2 ? "<mark>" + esc(part) + "</mark>" : esc(part);
    }).join("");
  }

  /* Lets long names wrap after . : and _ instead of in the middle of a word. */
  function breakable(html) {
    return html.replace(/([.:_])(?![^<]*>)/g, "$1<wbr>");
  }

  function paramHit(page, tok) {
    for (var paramIdx = 0; paramIdx < page.params.length; paramIdx++) {
      if (page.params[paramIdx].name.toLowerCase().indexOf(tok) !== -1) {
        return true;
      }
    }

    return false;
  }

  /* Every token has to match somewhere. Exact and leading name matches rank first, then name
     fragments, then the full id, then the category, summary and parameter names. */
  function score(page, toks) {
    var name = page.name.toLowerCase();
    var id = page.id.toLowerCase();
    var summary = page.summary.toLowerCase();
    var category = (page.category || "").toLowerCase();
    var total = 0;

    for (var tokIdx = 0; tokIdx < toks.length; tokIdx++) {
      var tok = toks[tokIdx];
      var points = 0;

      if (name === tok || id === tok) points = 100;
      else if (name.indexOf(tok) === 0) points = 80;
      else if (new RegExp("[.:_]" + reEsc(tok)).test(id)) points = 65;
      else if (name.indexOf(tok) !== -1) points = 50;
      else if (id.indexOf(tok) !== -1) points = 40;
      else if (category.indexOf(tok) !== -1) points = 25;
      else if (summary.indexOf(tok) !== -1) points = 20;
      else if (paramHit(page, tok)) points = 15;

      if (!points) {
        return 0;
      }

      total += points;
    }

    if (CONTAINERS[page.kind]) total += 3;
    if (page.deprecated) total -= 10;
    if (page.access === "internal") total -= 5;

    return Math.max(total, 1);
  }

  function searchPages(q, where) {
    var toks = tokens(q);
    var found = [];

    for (var pageIdx = 0; pageIdx < data.length; pageIdx++) {
      var page = data[pageIdx];

      if (page.lang !== where || !realmVisible(page)) {
        continue;
      }

      var points = score(page, toks);

      if (points) {
        found.push({ page: page, points: points });
      }
    }

    found.sort(function (a, b) {
      return b.points - a.points || (a.page.id < b.page.id ? -1 : 1);
    });

    return found.slice(0, 80).map(function (item) { return item.page; });
  }

  function byName(a, b) {
    if (a.name < b.name) return -1;
    if (a.name > b.name) return 1;
    return 0;
  }

  function ofKind(kind) {
    var out = [];

    for (var itemIdx = 0; itemIdx < data.length; itemIdx++) {
      if (data[itemIdx].lang === lang && data[itemIdx].kind === kind) {
        out.push(data[itemIdx]);
      }
    }

    out.sort(byName);

    return out;
  }

  function children(parent) {
    var out = [];

    for (var itemIdx = 0; itemIdx < data.length; itemIdx++) {
      var item = data[itemIdx];

      if (item.lang === parent.lang && item.parent === parent.id) {
        out.push(item);
      }
    }

    out.sort(byName);

    return out;
  }

  /* Top-level pages of the current language, grouped by category. Pages without one fall into
     a section named after their kind. */
  function sections() {
    var named = {};
    var categories = [];
    var itemIdx;

    for (itemIdx = 0; itemIdx < data.length; itemIdx++) {
      var page = data[itemIdx];

      if (page.lang !== lang || page.parent) {
        continue;
      }

      var title = page.category || SECTION_OF[lang][page.kind] || "Other";

      if (!named[title]) {
        named[title] = [];

        if (page.category) {
          categories.push(title);
        }
      }

      named[title].push(page);
    }

    categories.sort();
    var order = categories.slice();

    SECTION_ORDER[lang].concat(["Other"]).forEach(function (title) {
      if (named[title] && order.indexOf(title) === -1) {
        order.push(title);
      }
    });

    return order.map(function (title) {
      return { title: title, pages: named[title].sort(byName) };
    });
  }

  function accessPill(page) {
    var kind = page.access === "internal" ? "internal" : "public";
    var label = kind === "internal" ? "Internal" : "Public";

    return '<span class="access ' + kind + '">' + label + "</span>";
  }

  function langPill(page) {
    return '<span class="lang">' + LANG_LABELS[page.lang] + "</span>";
  }

  function link(page, label) {
    var active = currentKey() === keyOf(page) ? " active" : "";

    var dep = page.deprecated ? " dep" : "";

    return '<a class="nav-link' + active + dep + '" href="' + href(page) + '"><span>' + esc(label) + '</span><span class="badges">' + realms(page.realm) + "</span></a>";
  }

  function accessOf(page) {
    return page.access === "internal" ? "internal" : "public";
  }

  function splitAccess(pages) {
    var pub = [];
    var internal = [];
    var itemIdx;

    for (itemIdx = 0; itemIdx < pages.length; itemIdx++) {
      if (accessOf(pages[itemIdx]) === "internal") {
        internal.push(pages[itemIdx]);
      } else {
        pub.push(pages[itemIdx]);
      }
    }

    return { publicPages: pub, internalPages: internal };
  }

  function folder(title, pages, labelOf) {
    if (!pages.length) {
      return "";
    }

    var html = '<div class="folder"><span class="folder-name">' + esc(title) + "</span>";
    var itemIdx;

    for (itemIdx = 0; itemIdx < pages.length; itemIdx++) {
      var page = pages[itemIdx];
      html += link(page, labelOf(page));
    }

    html += "</div>";

    return html;
  }

  function group(title, pages, q) {
    var blocks = "";
    var here = currentPage();

    for (var itemIdx = 0; itemIdx < pages.length; itemIdx++) {
      var page = pages[itemIdx];
      var kids = children(page);
      var visible = [];
      var childIdx;

      for (childIdx = 0; childIdx < kids.length; childIdx++) {
        var child = kids[childIdx];

        if (child.kind === "hook") {
          continue;
        }

        if (realmVisible(child)) {
          visible.push(child);
        }
      }

      if (!realmVisible(page) && visible.length === 0) {
        continue;
      }

      // A free function or constant has nothing to fold open.
      if (!CONTAINERS[page.kind] && !visible.length) {
        blocks += link(page, page.name);

        continue;
      }

      var split = splitAccess(visible);
      var open = q || currentKey() === keyOf(page) || (here && here.lang === page.lang && here.parent === page.id);
      blocks += '<div class="branch' + (open ? " open" : "") + '">' + link(page, page.name) + '<div class="kids">';
      blocks += folder("Public", split.publicPages, function (child) { return child.name; });
      blocks += folder("Internal", split.internalPages, function (child) { return child.name; });
      blocks += "</div></div>";
    }

    if (!blocks) {
      return "";
    }

    return '<section class="group"><h2>' + esc(title) + "</h2>" + blocks + "</section>";
  }

  function hookGroup(q) {
    var hooks = ofKind("hook");
    var shown = [];
    var itemIdx;

    for (itemIdx = 0; itemIdx < hooks.length; itemIdx++) {
      if (!realmVisible(hooks[itemIdx])) {
        continue;
      }

      shown.push(hooks[itemIdx]);
    }

    var split = splitAccess(shown);
    var blocks = folder("Public", split.publicPages, function (page) { return page.parent + ":" + page.name; });
    blocks += folder("Internal", split.internalPages, function (page) { return page.parent + ":" + page.name; });

    if (!blocks) {
      return "";
    }

    return '<section class="group"><h2>Hooks</h2>' + blocks + "</section>";
  }

  function tags(page) {
    var html = "";

    if (page.deprecated) {
      html += '<span class="tag warn">Deprecated</span>';
    }

    if (page.since) {
      html += '<span class="tag">Since ' + esc(page.since) + "</span>";
    }

    return html;
  }

  function resultHtml(page, toks, index) {
    var cls = "result" + (index === selected ? " selected" : "") + (page.deprecated ? " dep" : "");

    return '<a class="' + cls + '" role="option" data-index="' + index + '" href="' + href(page) + '">' +
      '<span class="top"><span><span class="title">' + breakable(mark(titleOf(page), toks)) + "</span>" + tags(page) + '</span><span class="badges">' + realms(page.realm) + "</span></span>" +
      '<span class="sum"><span class="kind">' + esc(page.kind) + "</span>" + mark(page.summary, toks) + "</span></a>";
  }

  function renderResults(q) {
    var toks = tokens(q);
    shownResults = searchPages(q, lang);

    if (selected >= shownResults.length) {
      selected = 0;
    }

    if (!shownResults.length) {
      var other = otherLang(q);
      var elsewhere = searchPages(q, other).length;
      var where = usesRealms() ? " in the realms shown" : "";
      var hint = elsewhere ? ' <a href="#' + other + '">' + elsewhere + (elsewhere === 1 ? " match" : " matches") + " in " + LANG_LABELS[other] + "</a>" : "";
      nav.innerHTML = '<div class="results"><p class="none">Nothing in ' + LANG_LABELS[lang] + " matches that" + where + "." + hint + "</p></div>";

      return;
    }

    var html = '<div class="results" role="listbox">';

    for (var resultIdx = 0; resultIdx < shownResults.length; resultIdx++) {
      html += resultHtml(shownResults[resultIdx], toks, resultIdx);
    }

    nav.innerHTML = html + "</div>";
  }

  function renderNav() {
    var q = query();

    if (q) {
      renderResults(q);

      return;
    }

    shownResults = [];
    nav.innerHTML = sections().map(function (item) {
      return group(item.title, item.pages, "");
    }).join("") + hookGroup("");
  }

  function renderLangSwitch() {
    var html = "";

    LANGS.forEach(function (name) {
      var on = name === lang;
      html += '<a class="lang-tab' + (on ? " on" : "") + '" href="#' + name + '"' + (on ? ' aria-current="true"' : "") + ' title="The ' + LANG_LABELS[name] + ' reference">' + LANG_LABELS[name] + "<em>" + count(name) + "</em></a>";
    });

    langSwitch.innerHTML = html;
    brandSub.textContent = LANG_LABELS[lang] + " API";
  }

  function renderRealmFilter() {
    var html = "";

    if (!usesRealms()) {
      document.getElementById("realm-filter").innerHTML = "";

      return;
    }

    REALM_NAMES.forEach(function (name) {
      var on = realmState[name];
      html += '<button type="button" class="chip ' + name + (on ? " on" : "") + '" data-realm="' + name + '" aria-pressed="' + on + '" title="Show or hide ' + name + ' pages"><i></i>' + name.charAt(0).toUpperCase() + name.slice(1) + "</button>";
    });

    document.getElementById("realm-filter").innerHTML = html;
  }

  function toggleRealm(name) {
    var others = REALM_NAMES.filter(function (other) { return other !== name && realmState[other]; });

    // Keep at least one realm on, otherwise the whole reference would disappear.
    if (realmState[name] && !others.length) {
      return;
    }

    realmState[name] = !realmState[name];
    saveRealms();
    selected = 0;
    renderRealmFilter();
    renderNav();
  }

  function moveSelection(step) {
    if (!shownResults.length) {
      return;
    }

    selected = (selected + step + shownResults.length) % shownResults.length;
    renderResults(query());
    var current = nav.querySelector(".result.selected");

    if (current && current.scrollIntoView) {
      current.scrollIntoView({ block: "nearest" });
    }
  }

  /* Links every type name inside a type, so table<Entity>, Option<&Address> and Vector3[] all
     lead somewhere. Only pages of the current language are linked. */
  function typeHtml(ty) {
    if (!ty) {
      return "";
    }

    var re = /[A-Za-z_][A-Za-z0-9_]*/g;
    var out = "";
    var last = 0;
    var match;

    while ((match = re.exec(ty))) {
      var target = byKey[lang + "/" + match[0]];
      out += esc(ty.slice(last, match.index));

      if (target && (CONTAINERS[target.kind] || target.kind === "type")) {
        out += '<a class="type" href="' + href(target) + '">' + esc(match[0]) + "</a>";
      } else {
        out += esc(match[0]);
      }

      last = match.index + match[0].length;
    }

    return '<span class="type">' + out + esc(ty.slice(last)) + "</span>";
  }

  function luaArg(param) {
    var piece = "";

    if (param.ty) {
      piece += typeHtml(param.ty) + " ";
    }

    piece += esc(param.name);

    if (param.optional) {
      piece += " = nil";
    }

    return piece;
  }

  /* The receiver keeps its own spelling: &self, &mut self, self: Box<Self>. */
  function rustArg(param) {
    if (param.name === "self") {
      return esc(param.ty || "self");
    }

    if (!param.ty) {
      return esc(param.name);
    }

    return esc(param.name) + ": " + typeHtml(param.ty);
  }

  function argList(page) {
    var parts = [];
    var itemIdx;

    for (itemIdx = 0; itemIdx < page.params.length; itemIdx++) {
      var param = page.params[itemIdx];
      parts.push(page.lang === "lua" ? luaArg(param) : rustArg(param));
    }

    return parts.join(", ");
  }

  function kw(word) {
    return "<span class='kw'>" + word + "</span>";
  }

  function firstType(page) {
    return page.returns[0] && page.returns[0].ty ? typeHtml(page.returns[0].ty) : "";
  }

  function isTuple(params) {
    for (var itemIdx = 0; itemIdx < params.length; itemIdx++) {
      if (!/^\d+$/.test(params[itemIdx].name)) {
        return false;
      }
    }

    return true;
  }

  function signature(page) {
    if (page.lang === "wasm") return wasmSignature(page);

    return page.lang === "rust" ? rustSignature(page) : luaSignature(page);
  }

  /* An import reads like the function it is: chain.storage_get(key: i32, key_len: i32) -> i32. */
  function wasmSignature(page) {
    if (page.kind === "library") return kw("module") + " " + esc(page.name);

    var path = (page.parent ? esc(page.parent) + "." : "") + esc(page.name);
    var ty = firstType(page);

    if (page.kind === "field") return path + (ty ? ": " + ty : "");

    return path + "(" + argList(page) + ")" + (ty ? " -&gt; " + ty : "");
  }

  function luaSignature(page) {
    if (page.kind === "field") {
      var ty = page.returns[0] ? typeHtml(page.returns[0].ty) + " " : "";
      var head = page.parent ? esc(page.parent) + "." + esc(page.name) : esc(page.name);

      return ty + head;
    }

    if (page.kind === "library") {
      return esc(page.name);
    }

    if (page.kind === "class") {
      if (!page.params.length) {
        return esc(page.name);
      }

      return esc(page.name) + "(" + argList(page) + ")";
    }

    var name = page.kind === "method" || page.kind === "hook"
      ? esc(page.parent) + ":" + esc(page.name)
      : (page.parent ? esc(page.parent) + "." : "") + esc(page.name);

    return name + "(" + argList(page) + ")";
  }

  function rustSignature(page) {
    var path = esc(titleOf(page));
    var ty = firstType(page);

    if (page.kind === "module") return kw("mod") + " " + path;
    if (page.kind === "struct" || page.kind === "enum" || page.kind === "trait") return kw(page.kind) + " " + path;
    if (page.kind === "type") return kw("type") + " " + path + (ty ? " = " + ty : "");
    if (page.kind === "constant") return kw("const") + " " + path + (ty ? ": " + ty : "");
    if (page.kind === "field") return path + (ty ? ": " + ty : "");

    if (page.kind === "variant") {
      if (!page.params.length) {
        return path;
      }

      if (isTuple(page.params)) {
        return path + "(" + page.params.map(function (param) {
          return param.ty ? typeHtml(param.ty) : "_";
        }).join(", ") + ")";
      }

      return path + " { " + argList(page) + " }";
    }

    return kw("fn") + " " + path + "(" + argList(page) + ")" + rustReturns(page);
  }

  function rustReturns(page) {
    var types = page.returns.filter(function (item) { return item.ty; }).map(function (item) {
      return typeHtml(item.ty);
    });

    if (!types.length) {
      return "";
    }

    return " -&gt; " + (types.length === 1 ? types[0] : "(" + types.join(", ") + ")");
  }

  /* Ids already join a member to its parent the way its language does: Entity:spawn,
     ents.create, Tx::hash, Tx.from. */
  function titleOf(page) {
    return page.id;
  }

  function realms(realm) {
    if (!realm) {
      return "";
    }

    return realm.split(",").map(function (name) {
      return name === "shared" ? pill("client") + pill("server") : pill(name);
    }).join("");
  }

  function pill(realm) {
    var label = realm.charAt(0).toUpperCase() + realm.slice(1);

    return '<span class="realm ' + realm + '" title="' + label + '">' + label + "</span>";
  }

  function crumbs(page) {
    var html = '<a href="#' + page.lang + '">' + LANG_LABELS[page.lang] + "</a>";
    var parent = page.parent ? byKey[page.lang + "/" + page.parent] : null;

    if (parent) {
      html += ' <span>/</span> <a href="' + href(parent) + '">' + esc(page.parent) + "</a>";
    } else if (CONTAINERS[page.kind]) {
      html += " <span>/</span> <span>" + esc(page.kind) + "</span>";
    }

    if (!CONTAINERS[page.kind]) {
      html += " <span>/</span> <span>" + esc(page.name) + "</span>";
    }

    return html;
  }

  function paramTable(page) {
    // An undocumented receiver is already in the signature and says nothing more here.
    var rows = page.params.filter(function (param) { return param.name !== "self" || param.desc; });

    if (!rows.length) {
      return "";
    }

    var heading = page.kind === "class" ? "Constructor" : page.kind === "variant" ? "Fields" : "Arguments";
    var html = "<h3>" + heading + "</h3><table class='args'><thead><tr><th>Name</th><th>Type</th><th>Description</th></tr></thead><tbody>";
    var itemIdx;

    for (itemIdx = 0; itemIdx < rows.length; itemIdx++) {
      var param = rows[itemIdx];
      html += "<tr><td class='name'>" + esc(param.name);

      if (param.optional) {
        html += " <em class='opt'>optional</em>";
      }

      html += "</td><td>" + typeHtml(param.ty) + "</td><td>" + esc(param.desc) + "</td></tr>";
    }

    html += "</tbody></table>";

    return html;
  }

  function returnTable(page) {
    if (!page.returns.length) {
      return "";
    }

    var heading = page.kind === "field" || page.kind === "constant" || page.kind === "type" ? "Type" : "Returns";
    var html = "<h3>" + heading + "</h3><table class='args ret'><thead><tr><th>Type</th><th>Description</th></tr></thead><tbody>";
    var itemIdx;

    for (itemIdx = 0; itemIdx < page.returns.length; itemIdx++) {
      var item = page.returns[itemIdx];
      html += "<tr><td>" + typeHtml(item.ty) + "</td><td>" + esc(item.desc) + "</td></tr>";
    }

    html += "</tbody></table>";

    return html;
  }

  var KEYWORDS = { lua: {}, rust: {} };

  ("and break do else elseif end false for function goto if in local nil not or repeat return then true until while").split(" ").forEach(function (word) {
    KEYWORDS.lua[word] = true;
  });

  ("as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while").split(" ").forEach(function (word) {
    KEYWORDS.rust[word] = true;
  });

  var TOKENS = {
    lua: /(--\[(=*)\[[\s\S]*?\]\2\]|--[^\n]*|\[(=*)\[[\s\S]*?\]\3\]|"(?:\\[\s\S]|[^"\\\n])*"|'(?:\\[\s\S]|[^'\\\n])*'|0[xX][0-9a-fA-F]+|\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|[A-Za-z_][A-Za-z0-9_]*)/g,
    rust: /(\/\/[^\n]*|\/\*[\s\S]*?\*\/|b?"(?:\\[\s\S]|[^"\\])*"|b?'(?:\\.|[^'\\\n])'|0[xXbBoO][0-9a-fA-F_]+|\d[\d_]*(?:\.\d[\d_]*)?(?:[eE][+-]?\d+)?(?:[iu](?:8|16|32|64|128|size)|f32|f64)?|[A-Za-z_][A-Za-z0-9_]*!?)/g
  };

  function tokenKind(token, next, which) {
    if (which === "lua") {
      if (token.slice(0, 2) === "--") return "com";
      if (token.charAt(0) === '"' || token.charAt(0) === "'" || token.charAt(0) === "[") return "str";
    } else {
      if (token.slice(0, 2) === "//" || token.slice(0, 2) === "/*") return "com";
      if (/^b?["']/.test(token)) return "str";
    }

    if (/^\d/.test(token)) return "num";
    if (KEYWORDS[which][token]) return "kw";
    if (token.charAt(token.length - 1) === "!" || /^[ \t]*\(/.test(next)) return "fn";
    if (which === "rust" && /^[A-Z]/.test(token)) return "ty";

    return "";
  }

  function highlight(code, which) {
    var re = new RegExp(TOKENS[which].source, "g");
    var html = "";
    var last = 0;
    var match;

    while ((match = re.exec(code))) {
      html += esc(code.slice(last, match.index));
      var token = match[0];
      var end = match.index + token.length;
      var kind = tokenKind(token, code.slice(end, end + 8), which);

      html += kind ? "<span class='" + kind + "'>" + esc(token) + "</span>" : esc(token);
      last = end;
    }

    return html + esc(code.slice(last));
  }

  /* Pages this one lists, then pages that list this one, so a link written on one side shows
     on both. */
  function seeAlso(page) {
    var key = keyOf(page);
    var keys = (page.see_also || []).slice();
    var back = (referrers[key] || []).slice().sort();

    back.forEach(function (other) {
      if (keys.indexOf(other) === -1 && other !== key) {
        keys.push(other);
      }
    });

    if (!keys.length) {
      return "";
    }

    var html = "<h3>See also</h3><ul class='seealso'>";

    keys.forEach(function (other) {
      var target = byKey[other];

      if (!target) {
        html += "<li><span>" + esc(other) + "</span></li>";

        return;
      }

      var across = target.lang !== page.lang ? '<span class="tag">' + LANG_LABELS[target.lang] + "</span>" : "";
      html += '<li><a href="' + href(target) + '">' + esc(titleOf(target)) + "</a>" + across + "<span>" + esc(target.summary) + "</span></li>";
    });

    return html + "</ul>";
  }

  function memberSection(title, pages) {
    if (!pages.length) {
      return "";
    }

    var split = splitAccess(pages);

    if (!split.publicPages.length && !split.internalPages.length) {
      return "";
    }

    return "<h3>" + esc(title) + "</h3>" + memberFolder("Public", split.publicPages) + memberFolder("Internal", split.internalPages);
  }

  function memberFolder(title, pages) {
    if (!pages.length) {
      return "";
    }

    var html = '<h4 class="folder-title">' + esc(title) + "</h4><ul class='members'>";
    var itemIdx;

    for (itemIdx = 0; itemIdx < pages.length; itemIdx++) {
      var page = pages[itemIdx];
      html += "<li><span class='member-name'><a href='" + href(page) + "'>" + esc(page.name) + "</a>" + tags(page) + realms(page.realm) + "</span><span>" + esc(page.summary) + "</span></li>";
    }

    html += "</ul>";

    return html;
  }

  function members(page) {
    if (!CONTAINERS[page.kind]) {
      return "";
    }

    var kids = children(page);
    var html = "";

    MEMBER_SECTIONS.forEach(function (entry) {
      html += memberSection(entry[1], kids.filter(function (child) { return child.kind === entry[0]; }));
    });

    return html;
  }

  function article(page) {
    var html = "<article>";
    var eyebrow = (page.category ? esc(page.category) + " · " : "") + esc(page.kind);
    html += '<p class="crumbs">' + crumbs(page) + "</p>";
    html += '<div class="title-row"><div><p class="eyebrow">' + eyebrow + '</p><h1>' + breakable(esc(titleOf(page))) + '</h1></div><div class="realms">' + langPill(page) + accessPill(page) + realms(page.realm) + "</div></div>";

    if (page.deprecated) {
      var since = page.deprecated_since ? " since " + esc(page.deprecated_since) : "";
      html += '<p class="banner warn">Deprecated' + since + ". " + esc(page.deprecated) + "</p>";
    }

    if (page.unimplemented) {
      html += '<p class="banner bad">' + esc(page.unimplemented) + "</p>";
    }

    html += '<pre class="sig">' + signature(page) + "</pre>";
    html += '<p class="summary">' + esc(page.summary) + "</p>";

    if (page.since) {
      html += '<p class="since">Since ' + esc(page.since) + "</p>";
    }

    html += paramTable(page);
    html += returnTable(page);

    if (page.example) {
      html += "<h3>Example</h3><div class='codewrap'><button type='button' class='copy' aria-label='Copy the example'>Copy</button><pre class='code'>" + highlight(page.example, page.lang === "lua" ? "lua" : "rust") + "</pre></div>";
    }

    if (page.panics) {
      html += "<h3>" + PANICS[page.lang] + "</h3><p>" + esc(page.panics) + "</p>";
    }

    if (page.note) {
      html += "<h3>Note</h3><p>" + esc(page.note) + "</p>";
    }

    if (page.safety) {
      html += "<h3>Safety</h3><p>" + esc(page.safety) + "</p>";
    }

    html += seeAlso(page);
    html += members(page);
    html += "</article>";

    return html;
  }

  function cardRow(title, pages) {
    var html = "<h3>" + esc(title) + "</h3><div class='cards'>";
    var itemIdx;

    for (itemIdx = 0; itemIdx < pages.length; itemIdx++) {
      var page = pages[itemIdx];
      var size = children(page).length;
      var detail = CONTAINERS[page.kind] ? page.kind + " · " + size + (size === 1 ? " page" : " pages") : page.kind;
      html += '<a class="card" href="' + href(page) + '"><strong>' + esc(page.name) + "</strong><em>" + esc(detail) + "</em><p>" + esc(page.summary) + "</p></a>";
    }

    html += "</div>";

    return html;
  }

  function home() {
    var groups = sections();
    var html = "<article class='home'>";
    html += "<header class='hero'><p class='eyebrow'>Reference</p><h1>" + LANG_LABELS[lang] + " <em>Wiki</em></h1>";
    html += "<p class='lede'>" + esc(LEDES[lang]) + "</p><div class='rule'></div></header>";

    if (!groups.length) {
      html += "<div class='empty'>" + EMPTY[lang] + "</div>";
    }

    groups.forEach(function (item) {
      html += cardRow(item.title, item.pages);
    });

    html += "</article>";

    return html;
  }

  function missing(id) {
    return "<article><h1>Not found</h1><p class='summary'>No page named " + esc(id) + ".</p></article>";
  }

  function renderMain() {
    var here = route();
    var page = lookup(here);

    if (!here.id) {
      document.title = LANG_LABELS[lang] + " — " + TITLE;
      main.innerHTML = home();

      return;
    }

    if (!page) {
      document.title = "Not found";
      main.innerHTML = missing(here.id);

      return;
    }

    document.title = titleOf(page) + " — " + TITLE;
    main.innerHTML = article(page);
    window.scrollTo(0, 0);
  }

  function renderAll() {
    sync();
    renderLangSwitch();
    renderRealmFilter();
    renderNav();
    renderMain();
  }

  function applyTheme(theme) {
    if (theme === "light") {
      document.documentElement.setAttribute("data-theme", "light");
      themeButton.textContent = "Dark";
      themeButton.setAttribute("aria-pressed", "false");

      return;
    }

    document.documentElement.removeAttribute("data-theme");
    themeButton.textContent = "Light";
    themeButton.setAttribute("aria-pressed", "true");
  }

  applyTheme(document.documentElement.getAttribute("data-theme") === "light" ? "light" : "dark");
  themeButton.addEventListener("click", function () {
    var next = document.documentElement.getAttribute("data-theme") === "light" ? "dark" : "light";

    try {
      localStorage.setItem("wiki-theme", next);
    } catch (err) {}

    applyTheme(next);
  });
  search.addEventListener("input", function () {
    selected = 0;
    renderNav();
  });
  search.addEventListener("keydown", function (ev) {
    if (ev.key === "ArrowDown") {
      ev.preventDefault();
      moveSelection(1);
    } else if (ev.key === "ArrowUp") {
      ev.preventDefault();
      moveSelection(-1);
    } else if (ev.key === "Enter") {
      var target = shownResults[selected];

      if (target) {
        ev.preventDefault();
        location.hash = href(target);
        search.blur();
      }
    } else if (ev.key === "Escape") {
      search.value = "";
      search.blur();
      selected = 0;
      renderNav();
    }
  });
  document.getElementById("realm-filter").addEventListener("click", function (ev) {
    var chip = ev.target.closest ? ev.target.closest(".chip") : null;

    if (chip) {
      toggleRealm(chip.getAttribute("data-realm"));
    }
  });
  nav.addEventListener("mousemove", function (ev) {
    var row = ev.target.closest ? ev.target.closest(".result") : null;

    if (row) {
      var index = Number(row.getAttribute("data-index"));

      if (index !== selected) {
        var old = nav.querySelector(".result.selected");

        if (old) old.classList.remove("selected");
        row.classList.add("selected");
        selected = index;
      }
    }
  });
  main.addEventListener("click", function (ev) {
    var button = ev.target.closest ? ev.target.closest(".copy") : null;

    if (!button) {
      return;
    }

    var text = button.parentNode.querySelector("pre").textContent;
    var done = function () {
      button.textContent = "Copied";
      button.classList.add("done");
      setTimeout(function () {
        button.textContent = "Copy";
        button.classList.remove("done");
      }, 1400);
    };
    var fallback = function () {
      var area = document.createElement("textarea");
      area.value = text;
      area.style.position = "fixed";
      area.style.opacity = "0";
      document.body.appendChild(area);
      area.select();
      var copied = false;

      try {
        copied = document.execCommand("copy");
      } catch (err) {}

      document.body.removeChild(area);

      if (copied) {
        done();

        return;
      }

      // The browser refused: select the code so the keyboard shortcut works.
      var range = document.createRange();
      range.selectNodeContents(button.parentNode.querySelector("pre"));
      var selection = window.getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
      button.textContent = "Press Ctrl+C";
      setTimeout(function () {
        button.textContent = "Copy";
      }, 2200);
    };

    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(done, fallback);
    } else {
      fallback();
    }
  });
  window.addEventListener("hashchange", function () {
    renderAll();
    aside.classList.remove("open");
  });
  menu.addEventListener("click", function () {
    aside.classList.toggle("open");
  });
  document.addEventListener("keydown", function (ev) {
    if (ev.key === "/" && document.activeElement !== search) {
      ev.preventDefault();
      search.focus();
    }
  });
  renderAll();
})();
</script>
</body>
</html>
"####;

pub fn render(pages: &[Page]) -> String {
    let mut json = serde_json::to_string(pages).expect("wiki json");
    json = json.replace('<', "\\u003c");
    let head = HEAD.replacen("/*__WIKI_FONTS__*/", &font_css(), 1);
    let mut html = String::with_capacity(head.len() + json.len() + TAIL.len());
    html.push_str(&head);
    html.push_str(&json);
    html.push_str(TAIL);

    html
}

fn font_css() -> String {
    let mut css = String::new();
    css.push_str(&face(
        "Newsreader",
        "normal",
        400,
        include_bytes!("../fonts/Newsreader-Regular.woff2"),
    ));
    css.push_str(&face(
        "Newsreader",
        "italic",
        400,
        include_bytes!("../fonts/Newsreader-Italic.woff2"),
    ));
    css.push_str(&face(
        "Manrope",
        "normal",
        400,
        include_bytes!("../fonts/Manrope-Regular.woff2"),
    ));
    css.push_str(&face(
        "Manrope",
        "normal",
        600,
        include_bytes!("../fonts/Manrope-Semibold.woff2"),
    ));
    css.push_str(&face(
        "JetBrains Mono",
        "normal",
        400,
        include_bytes!("../fonts/JetBrainsMono-Regular.woff2"),
    ));

    css
}

fn face(family: &str, style: &str, weight: u16, bytes: &[u8]) -> String {
    format!(
        "@font-face{{font-family:\"{family}\";font-style:{style};font-weight:{weight};font-display:swap;src:url(\"data:font/woff2;base64,{}\") format(\"woff2\");}}",
        base64(bytes)
    )
}

fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    let mut idx = 0;

    while idx + 3 <= data.len() {
        let n = ((data[idx] as u32) << 16) | ((data[idx + 1] as u32) << 8) | data[idx + 2] as u32;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push(TABLE[(n & 63) as usize] as char);
        idx += 3;
    }

    let left = data.len() - idx;

    if left == 0 {
        return out;
    }

    let mut n = (data[idx] as u32) << 16;

    if left == 2 {
        n |= (data[idx + 1] as u32) << 8;
    }

    out.push(TABLE[((n >> 18) & 63) as usize] as char);
    out.push(TABLE[((n >> 12) & 63) as usize] as char);

    if left == 2 {
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push('=');
    } else {
        out.push('=');
        out.push('=');
    }

    out
}
