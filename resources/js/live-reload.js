// Live-reload client (implementation plan §2): long-polls mudl-server's
// `/wait?since=N` route and reloads the page as soon as the file's version
// advances past `N`, or simply polls again on a timeout. The starting
// version comes from the `data-mudl-version` attribute `mudl-server`'s
// document template sets on the mode wrapper div (see
// `mudl_server::document`), read here instead of an inline `<script>` so
// the page's CSP doesn't need `script-src 'unsafe-inline'`
// (`docs/SECURITY.md` Finding 3).

(function () {
  "use strict";

  function poll(since) {
    fetch("/wait?since=" + since)
      .then(function (response) {
        return response.json();
      })
      .then(function (body) {
        if (body.version !== since) {
          location.reload();
        } else {
          poll(body.version);
        }
      })
      .catch(function () {
        // The server went away (window closing, process exiting) or a
        // transient network hiccup — either way, stop polling rather than
        // spinning forever.
      });
  }

  var versionHost = document.querySelector("[data-mudl-version]");
  var startingVersion = versionHost
    ? parseInt(versionHost.getAttribute("data-mudl-version"), 10)
    : 0;
  poll(startingVersion);
})();
