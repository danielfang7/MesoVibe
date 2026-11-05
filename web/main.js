// Puts the download for the visitor's OS first and makes it the primary button. It runs as soon
// as the buttons exist, so a Mac visitor never sees them swap.
// Without this script (or on any other OS) Windows stays first, since most players are on Windows.
(function () {
  const box = document.getElementById("downloads");
  if (!box) return;
  const platform = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || "";
  // iPads report a Mac platform; a touch screen tells them apart.
  const isMac = /mac/i.test(platform) && navigator.maxTouchPoints <= 1;
  if (!isMac) return;
  const mac = box.querySelector('[data-os="mac"]');
  const windows = box.querySelector('[data-os="windows"]');
  mac.classList.replace("secondary", "primary");
  windows.classList.replace("primary", "secondary");
  box.prepend(mac);
})();

// The example Chat view's tabs. Opening a tab clears its unread count, as in the app.
// Arrow keys, Home and End move between tabs (the WAI-ARIA tabs pattern).
document.addEventListener("DOMContentLoaded", function () {
  const tabs = Array.from(document.querySelectorAll('.mock-chat [role="tab"]'));

  function select(tab) {
    for (const other of tabs) {
      const on = other === tab;
      other.setAttribute("aria-selected", String(on));
      other.tabIndex = on ? 0 : -1;
      document.getElementById(other.getAttribute("aria-controls")).hidden = !on;
    }
    const count = tab.querySelector(".count");
    if (count) count.remove();
  }

  for (const [i, tab] of tabs.entries()) {
    tab.addEventListener("click", () => select(tab));
    tab.addEventListener("keydown", (event) => {
      const next = {
        ArrowRight: tabs[(i + 1) % tabs.length],
        ArrowLeft: tabs[(i - 1 + tabs.length) % tabs.length],
        Home: tabs[0],
        End: tabs[tabs.length - 1],
      }[event.key];
      if (!next) return;
      event.preventDefault();
      select(next);
      next.focus();
    });
  }
});
