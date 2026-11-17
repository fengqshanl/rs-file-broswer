/*
 * Applies the persisted theme before the stylesheet paints, so a light-mode
 * user never sees a dark flash. Loaded synchronously from <head>.
 *
 * The authoritative copy lives in settings.json; the cookie is only a fast
 * path because the desktop front binds a random port on every launch.
 */
(function () {
  var root = document.documentElement;
  // Light is the shipped default; "auto" is still available as an opt-in from
  // the 视图 menu for users who want to follow the operating system.
  var choice = 'light';
  var cookie = document.cookie || '';
  var match = cookie.match(/(?:^|;\s*)rockbadger_theme=(auto|light|dark)/);
  if (!match) {
    match = cookie.match(/(?:^|;\s*)agentgod_theme=(auto|light|dark)/);
  }
  if (match) {
    choice = match[1];
  } else {
    try {
      var stored =
        localStorage.getItem('rockbadger.theme') ||
        localStorage.getItem('agentgod.theme');
      if (stored === 'light' || stored === 'dark' || stored === 'auto') {
        choice = stored;
      }
    } catch (error) {
      /* storage can be unavailable in a hardened webview */
    }
  }
  var light = false;
  try {
    light = window.matchMedia('(prefers-color-scheme: light)').matches;
  } catch (error) {
    /* very old WebKit */
  }
  var resolved = choice === 'auto' ? (light ? 'light' : 'dark') : choice;
  root.setAttribute('data-theme', resolved);
  root.setAttribute('data-theme-choice', choice);
  root.style.colorScheme = resolved;
})();
