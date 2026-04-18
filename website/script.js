// Copy-to-clipboard for install commands.
document.querySelectorAll("[data-copy]").forEach((btn) => {
  btn.addEventListener("click", async () => {
    const target = document.querySelector(btn.getAttribute("data-copy"));
    if (!target) return;
    const text = target.textContent.trim();
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      const range = document.createRange();
      range.selectNode(target);
      const sel = window.getSelection();
      sel.removeAllRanges();
      sel.addRange(range);
      document.execCommand("copy");
      sel.removeAllRanges();
    }
    const original = btn.textContent;
    btn.textContent = "Copied";
    btn.classList.add("is-copied");
    setTimeout(() => {
      btn.textContent = original;
      btn.classList.remove("is-copied");
    }, 1500);
  });
});

// Smooth-scroll for same-page anchors.
document.querySelectorAll('a[href^="#"]').forEach((link) => {
  link.addEventListener("click", (e) => {
    const id = link.getAttribute("href");
    if (id.length <= 1) return;
    const el = document.querySelector(id);
    if (!el) return;
    e.preventDefault();
    el.scrollIntoView({ behavior: "smooth", block: "start" });
  });
});

// Mobile drawer menu.
(() => {
  const toggle = document.querySelector("[data-nav-toggle]");
  const drawer = document.getElementById("nav-drawer");
  const backdrop = document.querySelector("[data-nav-backdrop]");
  const close = document.querySelector("[data-nav-close]");
  if (!toggle || !drawer || !backdrop) return;

  const open = () => {
    drawer.classList.add("is-open");
    backdrop.hidden = false;
    requestAnimationFrame(() => backdrop.classList.add("is-visible"));
    toggle.setAttribute("aria-expanded", "true");
    drawer.setAttribute("aria-hidden", "false");
    document.body.classList.add("nav-open");
  };

  const shut = () => {
    drawer.classList.remove("is-open");
    backdrop.classList.remove("is-visible");
    toggle.setAttribute("aria-expanded", "false");
    drawer.setAttribute("aria-hidden", "true");
    document.body.classList.remove("nav-open");
    setTimeout(() => {
      if (!drawer.classList.contains("is-open")) backdrop.hidden = true;
    }, 240);
  };

  toggle.addEventListener("click", () => {
    if (drawer.classList.contains("is-open")) shut();
    else open();
  });
  close?.addEventListener("click", shut);
  backdrop.addEventListener("click", shut);
  drawer.querySelectorAll("[data-nav-link]").forEach((a) => {
    a.addEventListener("click", shut);
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && drawer.classList.contains("is-open")) shut();
  });
  // If the viewport grows past the mobile breakpoint, close any open drawer.
  const mq = window.matchMedia("(min-width: 961px)");
  mq.addEventListener("change", (e) => {
    if (e.matches) shut();
  });
})();
