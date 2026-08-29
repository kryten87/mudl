// Live-reload client (implementation plan §2): long-polls mudl-server's
// `/wait?since=N` route and reloads the page as soon as the file's version
// advances past `N`, or simply polls again on a timeout. `MUDL_VERSION` is
// substituted by `mudl-server`'s document template with the version the
// page was served at (see `mudl_server::document`).

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

  poll(MUDL_VERSION);
})();
