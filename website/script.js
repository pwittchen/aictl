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

// macOS desktop download — picks the arch from the sibling select and
// hits the GitHub "latest release" redirect for the matching DMG asset.
document.querySelectorAll("[data-download-mac]").forEach((btn) => {
  btn.addEventListener("click", () => {
    const group = btn.closest(".download");
    const select = group?.querySelector("[data-arch-select]");
    const arch = select?.value || "aarch64";
    const url = `https://github.com/pwittchen/aictl/releases/latest/download/aictl-desktop-darwin-${arch}.dmg`;
    window.location.href = url;
  });
});

// Reveal preview blocks on scroll, staggering header → CTA → screenshot.
// No-ops gracefully without IntersectionObserver or under prefers-reduced-motion.
(() => {
  const targets = document.querySelectorAll(".preview-block--reveal, .surfaces-section--reveal");
  if (!targets.length) return;
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (reduced || typeof IntersectionObserver !== "function") {
    targets.forEach((el) => el.classList.add("is-visible"));
    return;
  }
  const io = new IntersectionObserver((entries) => {
    entries.forEach((entry) => {
      if (entry.isIntersecting) {
        entry.target.classList.add("is-visible");
        io.unobserve(entry.target);
      }
    });
  }, { threshold: 0.05, rootMargin: "0px 0px -10% 0px" });
  targets.forEach((el) => io.observe(el));
})();

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

// Animated CLI demo. Replays a real interaction — user types a query,
// assistant streams reasoning, runs a tool, prints results, streams the
// answer, then waits at a fresh prompt. Loops. Falls back to the static
// HTML on prefers-reduced-motion or if the markup is missing.
(() => {
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  const demos = document.querySelectorAll(".cli-demo[data-cli-animate]");
  if (!demos.length) return;

  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const inView = (el) => {
    const r = el.getBoundingClientRect();
    return r.top < window.innerHeight && r.bottom > 0;
  };
  const mk = (tag, cls) => {
    const el = document.createElement(tag);
    if (cls) el.className = cls;
    return el;
  };

  demos.forEach((demo) => {
    if (inView(demo)) start(demo);
    else if (typeof IntersectionObserver === "function") {
      const io = new IntersectionObserver((entries, o) => {
        entries.forEach((e) => {
          if (e.isIntersecting) {
            o.disconnect();
            start(demo);
          }
        });
      }, { threshold: 0.1 });
      io.observe(demo);
    } else {
      start(demo);
    }
  });

  function start(demo) {
    const body = demo.querySelector(".cli-demo__body");
    if (!body) return;
    const banner = demo.querySelector(".cli-demo__banner");

    // Lock body height to its current static-rendered size so loop resets
    // don't collapse surrounding layout.
    body.style.minHeight = body.offsetHeight + "px";

    Array.from(body.children).forEach((c) => {
      if (c !== banner) c.remove();
    });

    const userInput = mk("div", "cli-demo__input cli-demo__input--user");
    userInput.setAttribute("aria-hidden", "true");
    const userChev = mk("span", "cli-demo__chevron");
    userChev.textContent = "❯";
    const userText = mk("span");
    const userCursor = mk("span", "cli-demo__cursor");
    userInput.append(userChev, userText, userCursor);

    const convo = mk("pre", "cli-demo__convo");
    convo.setAttribute("aria-hidden", "true");

    const nextInput = mk("div", "cli-demo__input cli-demo__input--next");
    nextInput.setAttribute("aria-hidden", "true");
    const nextChev = mk("span", "cli-demo__chevron");
    nextChev.textContent = "❯";
    const nextCursor = mk("span", "cli-demo__cursor");
    nextInput.append(nextChev, nextCursor);
    nextInput.style.visibility = "hidden";

    body.append(userInput, convo, nextInput);

    play({ userText, userCursor, convo, nextInput, demo });
  }

  async function typeText(target, text, perChar, jitter) {
    for (const ch of text) {
      target.append(document.createTextNode(ch));
      await sleep(perChar + Math.random() * jitter);
    }
  }

  function appendHTML(target, html) {
    target.insertAdjacentHTML("beforeend", html);
  }

  async function waitVisible(demo) {
    while (!inView(demo) || document.hidden) await sleep(500);
  }

  async function play(parts) {
    const { userText, userCursor, convo, nextInput, demo } = parts;

    await waitVisible(demo);
    await sleep(700);

    // 1. User types the query.
    await typeText(userText, "describe in one sentence contents of this dir", 55, 35);
    await sleep(450);

    // 2. Submit — drop the inline cursor, the line stays as history.
    userCursor.style.display = "none";
    await sleep(320);

    // 3. Assistant reasoning streams in.
    appendHTML(convo, "  ");
    await typeText(convo, "Let me look at the directory contents.", 22, 18);
    appendHTML(convo, "\n\n");
    await sleep(260);

    // 4. Tool call header.
    appendHTML(convo, '  <span class="tt-tool">list_directory</span> <span class="tt-rule">──</span>\n\n');
    await sleep(280);

    // 5. Tool output, line by line (feels like a real listing scrolling).
    const entries = [
      ["[FILE]", "Cargo.toml"],
      ["[FILE]", ".DS_Store"],
      ["[FILE]", "LICENSE"],
      ["[DIR] ", ".aictl"],
      ["[FILE]", "AICTL.md"],
      ["[DIR] ", "target"],
      ["[FILE]", "install.sh"],
      ["[DIR] ", "tests"],
      ["[DIR] ", "website"],
      ["[DIR] ", ".claude"],
    ];
    for (const [tag, name] of entries) {
      appendHTML(convo, `  <span class="tt-tag">${tag}</span> ${name}\n`);
      await sleep(60);
    }
    appendHTML(convo, '  <span class="tt-dim">… 11 lines hidden …</span>\n');
    await sleep(110);
    appendHTML(convo, '  <span class="tt-tag">[FILE]</span> CLAUDE.md\n');
    await sleep(60);
    appendHTML(convo, '  <span class="tt-tag">[DIR] </span> src\n\n');
    await sleep(280);

    // 6. Per-turn cost meter.
    appendHTML(convo, '  <span class="tt-rule">───────────────────────────────────────────────────────────────────────────</span>\n');
    appendHTML(convo, '  <span class="tt-status">claude-sonnet-4 · 3↑ (4320⚡) · 69↓ · 1 tool(s) · $0.0024 · 3.2s · ctx 1%</span>\n\n');
    await sleep(380);

    // 7. Final answer streams.
    appendHTML(convo, "  ");
    await typeText(convo, "This is the ", 22, 14);
    appendHTML(convo, '<span class="tt-bold">aictl</span>');
    await typeText(
      convo,
      " Rust CLI project — an AI agent tool with source code,\n  documentation (README, ARCH, ROADMAP), pricing references, a website, examples,\n  tests, GitHub CI config, and an install script.\n\n",
      18,
      12
    );
    await sleep(280);

    // 8. Final per-turn meter and session summary.
    appendHTML(convo, '  <span class="tt-rule">───────────────────────────────────────────────────────────────────────────</span>\n');
    appendHTML(convo, '  <span class="tt-status">claude-sonnet-4 · 221↑ (4320⚡) · 53↓ · 1 tool(s) · $0.0031 · 2.3s · ctx 2%</span>\n\n');
    await sleep(220);
    appendHTML(convo, '  <span class="tt-rule">───────────────────────────────────────────────────────────────────────────</span>\n');
    appendHTML(convo, '  <span class="tt-summary">2 reqs · 1 tool(s) · 224↑ · 122↓ · $0.0054 · 6.3s · ctx 2%</span>');
    await sleep(450);

    // 9. New prompt — agent waiting for the next turn. Stays as the
    //    final resting state; the animation does not loop.
    nextInput.style.visibility = "";
  }
})();

// Animated desktop-app demo. Same idea as the CLI demo — user types into
// the composer, the assistant streams reasoning, runs a tool, lists files,
// streams the answer, and the composer rests on the empty placeholder.
// Plays once. Falls back to the static HTML on prefers-reduced-motion.
(() => {
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  const demos = document.querySelectorAll(".dt-demo[data-dt-animate]");
  if (!demos.length) return;

  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const inView = (el) => {
    const r = el.getBoundingClientRect();
    return r.top < window.innerHeight && r.bottom > 0;
  };
  const mk = (tag, cls) => {
    const el = document.createElement(tag);
    if (cls) el.className = cls;
    return el;
  };

  demos.forEach((demo) => {
    if (inView(demo)) start(demo);
    else if (typeof IntersectionObserver === "function") {
      const io = new IntersectionObserver((entries, o) => {
        entries.forEach((e) => {
          if (e.isIntersecting) {
            o.disconnect();
            start(demo);
          }
        });
      }, { threshold: 0.05 });
      io.observe(demo);
    } else {
      start(demo);
    }
  });

  function start(demo) {
    const messages = demo.querySelector(".dt-demo__messages");
    const textarea = demo.querySelector(".dt-demo__textarea");
    const toolbarCount = demo.querySelector(".dt-demo__toolbar-count");
    if (!messages || !textarea) return;

    // Lock min-height so loop reset doesn't collapse surrounding layout.
    messages.style.minHeight = messages.offsetHeight + "px";

    // Tear down static state — we'll rebuild via the animation.
    messages.innerHTML = "";
    textarea.innerHTML = "";

    const placeholder = mk("span", "dt-demo__placeholder");
    placeholder.textContent = "Type a message";
    const typed = mk("span", "dt-demo__typed");
    typed.style.color = "var(--dt-fg)";
    const caret = mk("span", "dt-demo__caret");
    textarea.append(placeholder, typed, caret);

    if (toolbarCount) toolbarCount.textContent = "· 0 messages";

    play({ messages, placeholder, typed, toolbarCount, demo });
  }

  async function typeText(target, text, perChar, jitter) {
    for (const ch of text) {
      target.append(document.createTextNode(ch));
      await sleep(perChar + Math.random() * jitter);
    }
  }

  function appendHTML(target, html) {
    target.insertAdjacentHTML("beforeend", html);
  }

  async function waitVisible(demo) {
    while (!inView(demo) || document.hidden) await sleep(500);
  }

  function setCount(target, n) {
    if (!target) return;
    target.textContent = `· ${n} message${n === 1 ? "" : "s"}`;
  }

  function makeMsg(role, label) {
    const article = mk("article", "dt-demo__msg");
    article.dataset.role = role;
    const meta = mk("div", "dt-demo__msg-meta");
    meta.textContent = label;
    const body = mk("div", "dt-demo__msg-body");
    article.append(meta, body);
    return { article, body };
  }

  async function play(parts) {
    const { messages, placeholder, typed, toolbarCount, demo } = parts;
    const scroll = () => { messages.scrollTop = messages.scrollHeight; };
    let count = 0;

    await waitVisible(demo);
    await sleep(700);

    // 1. User types into the composer.
    placeholder.style.display = "none";
    await typeText(typed, "describe in one sentence contents of this dir", 55, 35);
    await sleep(450);

    // 2. Submit — composer clears, user message lands in history.
    typed.textContent = "";
    placeholder.style.display = "";
    const userMsg = makeMsg("user", "USER");
    userMsg.body.textContent = "describe in one sentence contents of this dir";
    messages.append(userMsg.article);
    setCount(toolbarCount, ++count);
    scroll();
    await sleep(420);

    // 3. Assistant reasoning streams in.
    const reasoningMsg = makeMsg("assistant", "ASSISTANT");
    messages.append(reasoningMsg.article);
    setCount(toolbarCount, ++count);
    await typeText(reasoningMsg.body, "Let me look at the directory contents.", 22, 18);
    scroll();
    await sleep(280);

    // 4. Tool block — header, then file lines streaming in.
    const tool = mk("div", "dt-demo__tool");
    tool.setAttribute("aria-hidden", "true");
    const head = mk("div", "dt-demo__tool-head");
    const tag = mk("span", "dt-demo__tool-tag"); tag.textContent = "TOOL";
    const name = mk("span", "dt-demo__tool-name"); name.textContent = "list_directory";
    head.append(tag, name);
    const toolBody = mk("pre", "dt-demo__tool-body");
    tool.append(head, toolBody);
    messages.append(tool);
    scroll();
    await sleep(260);

    const files = [
      "[FILE] Cargo.toml",
      "[FILE] LICENSE",
      "[DIR]  .aictl",
      "[FILE] AICTL.md",
      "[DIR]  target",
      "[FILE] install.sh",
      "[DIR]  tests",
      "[DIR]  website",
      "[DIR]  .claude",
      "[FILE] CLAUDE.md",
      "[DIR]  src",
    ];
    for (let i = 0; i < files.length; i++) {
      const line = files[i] + (i < files.length - 1 ? "\n" : "");
      toolBody.append(document.createTextNode(line));
      scroll();
      await sleep(70);
    }
    await sleep(380);

    // 5. Final assistant answer streams.
    const answerMsg = makeMsg("assistant", "ASSISTANT");
    messages.append(answerMsg.article);
    setCount(toolbarCount, ++count);
    await typeText(answerMsg.body, "This directory is the root of the ", 18, 12);
    appendHTML(answerMsg.body, "<strong>aictl</strong>");
    await typeText(answerMsg.body, " Rust workspace, containing source crates (", 16, 10);
    appendHTML(answerMsg.body, "<code>aictl-cli</code>, <code>aictl-core</code>, <code>aictl-server</code>, <code>aictl-desktop</code>");
    await typeText(answerMsg.body, "), docs, install scripts, and standard project files.", 16, 10);
    scroll();

    // 6. Composer's already showing placeholder + caret — that becomes
    //    the final resting state. Animation does not loop.
  }
})();

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
