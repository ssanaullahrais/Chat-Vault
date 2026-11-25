// Theme toggle and live GitHub figures. Everything works without this script.
(function () {
  var root = document.documentElement;
  var btn = document.getElementById("theme-toggle");
  if (btn) {
    btn.addEventListener("click", function () {
      var dark = root.getAttribute("data-theme") === "dark" ||
        (!root.getAttribute("data-theme") && window.matchMedia("(prefers-color-scheme: dark)").matches);
      var next = dark ? "light" : "dark";
      root.setAttribute("data-theme", next);
      try { localStorage.setItem("theme", next); } catch (e) {}
    });
  }

  var y = document.getElementById("year");
  if (y) y.textContent = String(new Date().getFullYear());

  // Star count and latest version from the public GitHub API (no sign-in, nothing about the visitor is sent).
  var stars = document.querySelectorAll("[data-gh-stars]");
  var version = document.querySelectorAll("[data-gh-version]");
  if (!stars.length && !version.length) return;
  var repo = "https://api.github.com/repos/ssanaullahrais/Chat-Vault";
  function fmt(n) { return n >= 1000 ? (n / 1000).toFixed(n >= 10000 ? 0 : 1) + "k" : String(n); }
  if (stars.length && window.fetch) {
    fetch(repo).then(function (r) { return r.ok ? r.json() : null; }).then(function (d) {
      if (!d || typeof d.stargazers_count !== "number") return;
      stars.forEach(function (el) { el.textContent = fmt(d.stargazers_count); el.hidden = false; });
    }).catch(function () {});
  }
  if (version.length && window.fetch) {
    fetch(repo + "/releases/latest").then(function (r) { return r.ok ? r.json() : null; }).then(function (d) {
      if (!d || !d.tag_name) return;
      version.forEach(function (el) { el.textContent = d.tag_name; el.hidden = false; });
    }).catch(function () {});
  }
})();
