/** Locale from GET /api/locales/{code} — English fallback until fetch completes. */
(function () {
  const LANG_KEY = "qb-lang";
  const EN = window.__qbUiEn || {};
  let ui = Object.assign({}, EN);
  let currentLang = "en";
  let localeReady = false;

  function isTag(value) {
    return typeof value === "string" && /^[a-z]{2}$/.test(value);
  }

  function savedLang() {
    try {
      const saved = localStorage.getItem(LANG_KEY);
      return isTag(saved) ? saved : null;
    } catch (_) {
      return null;
    }
  }

  function browserLang() {
    const list =
      navigator.languages && navigator.languages.length
        ? navigator.languages
        : [navigator.language || "en"];
    for (const raw of list) {
      const primary = String(raw || "")
        .trim()
        .toLowerCase()
        .replace(/_/g, "-")
        .split("-")[0];
      if (isTag(primary)) return primary;
    }
    return "en";
  }

  async function systemLang() {
    try {
      const res = await fetch("/api/system-locale", { cache: "no-store" });
      if (!res.ok) return null;
      const data = await res.json();
      return isTag(data && data.lang) ? data.lang : null;
    } catch (_) {
      return null;
    }
  }

  async function resolveInitial() {
    return savedLang() || (await systemLang()) || browserLang();
  }

  function t(key, vars) {
    let s = ui[key] || EN[key] || key;
    if (!vars) return s;
    for (const [k, v] of Object.entries(vars)) {
      s = s.split("{" + k + "}").join(String(v));
    }
    return s;
  }

  const THEME_KEY = "qb-theme";

  function savedTheme() {
    try {
      const saved = localStorage.getItem(THEME_KEY);
      return saved === "dark" || saved === "light" ? saved : null;
    } catch (_) {
      return null;
    }
  }

  function applyThemeLabel() {
    const btn = document.getElementById("theme-toggle");
    if (!btn) return;
    const dark = document.documentElement.getAttribute("data-theme") === "dark";
    btn.textContent = t(dark ? "theme_to_light" : "theme_to_dark");
    btn.title = t(dark ? "theme_to_light_title" : "theme_to_dark_title");
    btn.setAttribute("aria-pressed", dark ? "true" : "false");
  }

  function setTheme(mode, persist) {
    document.documentElement.setAttribute("data-theme", mode);
    if (persist) {
      try {
        localStorage.setItem(THEME_KEY, mode);
      } catch (_) {}
    }
    applyThemeLabel();
  }

  function applyStatic() {
    const title = document.getElementById("page-title");
    const lede = document.getElementById("lede");
    const refresh = document.getElementById("refresh");
    const settingsOpen = document.getElementById("settings-open");
    if (title) title.textContent = t("page_title");
    if (lede) lede.textContent = t("lede");
    if (refresh) refresh.textContent = t("refresh");
    if (settingsOpen) settingsOpen.textContent = t("settings");
    applyThemeLabel();
    const dlg = document.getElementById("settings-dialog");
    if (dlg) updateSettingsChrome(dlg);
  }

  function updateSettingsChrome(dlg) {
    if (!dlg) return;
    const h = dlg.querySelector("#settings-title");
    if (h) h.textContent = t("settings_title");
    const map = [
      ["settings-lang-label", "settings_language"],
      ["settings-refresh-label", "settings_refresh_interval"],
      ["settings-timeout-label", "settings_provider_timeout"],
      ["settings-cache-label", "settings_cache_ttl"],
      ["settings-server-label", "settings_server"],
      ["settings-hint-bind", "settings_bind_hint"],
      ["settings-save", "settings_save"],
      ["settings-close", "settings_close"],
    ];
    for (const [id, key] of map) {
      const el = dlg.querySelector("#" + id);
      if (!el) continue;
      if (el.tagName === "BUTTON") el.textContent = t(key);
      else if (id.endsWith("-label") || id === "settings-hint-bind") el.textContent = t(key);
    }
  }

  function applyDir(dir) {
    const value = dir === "rtl" ? "rtl" : "ltr";
    document.documentElement.dir = value;
    const root = document.getElementById("html-root");
    if (root) root.dir = value;
  }

  async function loadTag(tag) {
    const res = await fetch("/api/locales/" + encodeURIComponent(tag), {
      cache: "no-store",
    });
    if (!res.ok) return null;
    return res.json();
  }

  async function setLang(lang, opts) {
    const persist = !!(opts && opts.persist);
    const silent = opts && opts.silent;
    let tag = isTag(lang) ? lang : "en";
    try {
      let data = await loadTag(tag);
      if (!data && tag !== "en") {
        tag = "en";
        data = await loadTag(tag);
      }
      if (!data) {
        ui = Object.assign({}, EN);
        currentLang = "en";
        applyDir("ltr");
        applyStatic();
        return false;
      }
      ui = Object.assign({}, EN, data.ui || {});
      currentLang = isTag(data.lang) ? data.lang : tag;
      if (persist) localStorage.setItem(LANG_KEY, currentLang);
      document.documentElement.lang = currentLang;
      const root = document.getElementById("html-root");
      if (root) root.lang = currentLang;
      applyDir(data.dir);
      localeReady = true;
      applyStatic();
      if (!silent && typeof window.__qbOnLocale === "function") {
        await window.__qbOnLocale(currentLang);
      }
      return true;
    } catch (_) {
      ui = Object.assign({}, EN);
      currentLang = "en";
      applyDir("ltr");
      applyStatic();
      return false;
    }
  }

  function acceptLanguage() {
    return currentLang || "en";
  }

  document.documentElement.lang = currentLang;
  applyStatic();

  const themeToggle = document.getElementById("theme-toggle");
  if (themeToggle) {
    themeToggle.addEventListener("click", () => {
      const dark = document.documentElement.getAttribute("data-theme") === "dark";
      setTheme(dark ? "light" : "dark", true);
    });
  }

  const themeQuery = window.matchMedia("(prefers-color-scheme: dark)");
  const followSystemTheme = () => {
    if (savedTheme()) return;
    setTheme(themeQuery.matches ? "dark" : "light", false);
  };
  if (themeQuery.addEventListener) themeQuery.addEventListener("change", followSystemTheme);
  else if (themeQuery.addListener) themeQuery.addListener(followSystemTheme);

  window.qbI18n = {
    t,
    setLang,
    detectLang() {
      return savedLang() || browserLang();
    },
    acceptLanguage,
    get lang() {
      return currentLang;
    },
    get ready() {
      return localeReady;
    },
    async init() {
      const lang = await resolveInitial();
      return setLang(lang, { silent: false, persist: false });
    },
  };
})();
