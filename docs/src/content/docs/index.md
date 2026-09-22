---
title: niminal
description: A native coding agent for your repository. Fast startup, low memory, and idle CPU until you ask.
template: splash
hero:
  title: niminal
  tagline: Fast to start, light on memory, no CPU until you ask.
  actions:
    - text: Install
      link: /guides/install/
      variant: primary
      icon: right-arrow
    - text: Quickstart
      link: /guides/quickstart/
      variant: secondary
      icon: right-arrow
    - text: View on GitHub
      link: https://github.com/martineastwood/niminal
      variant: secondary
      icon: external
---

<div class="landing-shell not-content">
  <p class="landing-lede">Point niminal at a repo, describe the change, and it inspects, edits, runs the commands you approve, and keeps the session. Extend it in any language so it works the way you do.</p>

  <section class="landing-terminal" aria-labelledby="landing-terminal-title">
    <div class="landing-terminal-bar">
      <div class="landing-terminal-dots" aria-hidden="true"><span></span><span></span><span></span></div>
      <span id="landing-terminal-title">your-project</span>
    </div>
    <pre class="not-content"><code><span class="landing-prompt">$</span> niminal
<span class="landing-input">› Fix the failing parser test and run the focused test.</span>
<span class="landing-muted">read</span>   src/parser.cpp, tests/parser_test.cpp
<span class="landing-muted">edit</span>   Apply the smallest safe change
<span class="landing-muted">bash</span>   ./dev check --fast
<span class="landing-success">done   The focused test passes.</span></code></pre>
  </section>

  <section class="landing-section" aria-labelledby="landing-runtime-title">
    <p class="landing-kicker">Native binary</p>
    <h2 id="landing-runtime-title">A small process you can keep close to the work</h2>
    <p class="landing-section-intro">niminal compiles to a native C++ binary. Startup is fast, memory stays low, and the process waits on you, the provider, or a subprocess instead of polling.</p>
    <div class="landing-grid">
      <article class="landing-card">
        <span class="landing-card-index">01</span>
        <h3>Small process</h3>
        <p>Compiled to native code, not interpreted. You can run several niminal processes without a heavy runtime behind each one.</p>
      </article>
      <article class="landing-card">
        <span class="landing-card-index">02</span>
        <h3>Idle when waiting</h3>
        <p>It waits on you, a provider, or a subprocess. No polls, no timers, no idle CPU.</p>
      </article>
      <article class="landing-card">
        <span class="landing-card-index">03</span>
        <h3>Your provider, your keys</h3>
        <p>Requests go straight to the API you pick. Credentials stay in your environment, your local auth file, or a one-process <code>--api-key</code> override.</p>
      </article>
      <article class="landing-card">
        <span class="landing-card-index">04</span>
        <h3>Parallel-friendly</h3>
        <p>Run independent processes for separate workspaces, branches, or jobs. Each one has its own session and queues.</p>
      </article>
    </div>
  </section>

  <section class="landing-section" aria-labelledby="landing-work-title">
    <p class="landing-kicker">In your repository</p>
    <h2 id="landing-work-title">From a task to a tested change</h2>
    <p class="landing-section-intro">Ask for the outcome you want. niminal gathers context from the project, makes the smallest edit that fits, and runs the checks you approve.</p>
    <div class="landing-grid">
      <article class="landing-card">
        <span class="landing-card-index">01</span>
        <h3>See the whole workspace</h3>
        <p>Read files, search by pattern, and attach the context that matters instead of pasting files in.</p>
      </article>
      <article class="landing-card">
        <span class="landing-card-index">02</span>
        <h3>Smallest useful change</h3>
        <p>Read before edit, use version tokens from <code>read</code>, and keep unrelated work intact.</p>
      </article>
      <article class="landing-card">
        <span class="landing-card-index">03</span>
        <h3>You decide what runs</h3>
        <p>Reads, searches, and workspace edits run freely. Shell commands and extension tools ask the first time, with per-session and per-project grants.</p>
      </article>
      <article class="landing-card">
        <span class="landing-card-index">04</span>
        <h3>Come back later</h3>
        <p>Queue the next thought while a turn runs, resume a saved session, and let compaction make room for longer tasks.</p>
      </article>
    </div>
  </section>

  <section class="landing-section" aria-labelledby="landing-customize-title">
    <p class="landing-kicker">Extend it</p>
    <h2 id="landing-customize-title">Any language. No SDK.</h2>
    <p class="landing-section-intro">An extension is a program niminal starts and keeps running. Python, Go, Rust, JavaScript, a shell script: if it can read stdin and write stdout, it can add tools, slash commands, hooks, and live status. There is no client library and no language lock-in.</p>
    <div class="landing-extend">
      <div class="landing-code">
        <div class="landing-code-bar"><span>.niminal/extensions/hello/extension.py</span></div>
        <pre class="not-content"><code><span class="landing-muted">#!/usr/bin/env python3</span>
<span class="kw">import</span> json, sys&#10;
<span class="kw">def</span> send(value):
    print(json.dumps(value), flush=True)&#10;
send({
  type: <span class="str">'register'</span>,
  commands: [{ name: <span class="str">'hello'</span>, description: <span class="str">'Say hello'</span> }],
  tools: [{
    name: <span class="str">'hello_tool'</span>,
    description: <span class="str">'Return a greeting.'</span>,
    input_schema: { type: <span class="str">'object'</span> },
    capabilities: [<span class="str">'read'</span>],
  }],
})&#10;
<span class="kw">for</span> line <span class="kw">in</span> sys.stdin:
  message = json.loads(line)
  <span class="kw">if</span> message[<span class="str">'type'</span>] == <span class="str">'shutdown'</span>: <span class="kw">break</span></code></pre>
      </div>
      <div class="landing-grid">
        <article class="landing-card">
          <span class="landing-card-index">01</span>
          <h3>Stay in the language you ship</h3>
          <p>JSON lines on stdin and stdout. No plugin host, no bundle step, no FFI into the agent process.</p>
        </article>
        <article class="landing-card">
          <span class="landing-card-index">02</span>
          <h3>Spawn a tool, or keep a process</h3>
          <p>A <code>tool.json</code> executable runs per call. An extension stays up, sees session events, and can push status whenever it likes.</p>
        </article>
        <article class="landing-card">
          <span class="landing-card-index">03</span>
          <h3>Commands, tools, and hooks</h3>
          <p>Register slash commands, model tools, and lifecycle hooks from the same program. Reply when you have something to change.</p>
        </article>
        <article class="landing-card">
          <span class="landing-card-index">04</span>
          <h3>Files next to the work</h3>
          <p>Put extensions in <code>.niminal/extensions</code> or the portable <code>~/.agents/extensions</code> layout other agents already share.</p>
        </article>
      </div>
    </div>
    <div class="landing-links">
      <a href="/guides/extensions-and-hooks/" class="landing-link"><span>Persistent extensions</span><small>A long-running program in any language: tools, commands, hooks, and live status.</small><span aria-hidden="true">↗</span></a>
      <a href="/guides/external-tools/" class="landing-link"><span>External tools</span><small>Expose a one-shot executable as a typed model tool.</small><span aria-hidden="true">↗</span></a>
      <a href="/guides/skills/" class="landing-link"><span>Skills</span><small>Load Markdown procedures only when a task calls for them.</small><span aria-hidden="true">↗</span></a>
      <a href="/guides/prompt-templates/" class="landing-link"><span>Prompt templates</span><small>Turn recurring requests into slash commands.</small><span aria-hidden="true">↗</span></a>
      <a href="/guides/instructions/" class="landing-link"><span>Instructions</span><small>Set project and personal rules that reach every request.</small><span aria-hidden="true">↗</span></a>
    </div>
  </section>

  <section class="landing-section landing-split" aria-labelledby="landing-interfaces-title">
    <div>
      <p class="landing-kicker">One agent, several surfaces</p>
      <h2 id="landing-interfaces-title">Use the interface that fits the job</h2>
      <p class="landing-section-intro">The same agent is an interactive TUI, a one-shot command, a JSON event stream, or a long-running RPC process.</p>
    </div>
    <div class="landing-links">
      <a href="/guides/interactive-tui/" class="landing-link"><span>TUI</span><small>Work in the terminal with queues, mentions, and shortcuts.</small><span aria-hidden="true">↗</span></a>
      <a href="/guides/quickstart/" class="landing-link"><span>Print mode</span><small>Run one turn and keep stdout to the final answer, including from a pipe.</small><span aria-hidden="true">↗</span></a>
      <a href="/reference/json-mode/" class="landing-link"><span>JSON mode</span><small>Emit versioned JSONL events for a single run.</small><span aria-hidden="true">↗</span></a>
      <a href="/reference/rpc-mode/" class="landing-link"><span>RPC mode</span><small>Drive a long-running process with prompts, steering, follow-ups, and interrupts.</small><span aria-hidden="true">↗</span></a>
    </div>
  </section>

  <section class="landing-start" aria-labelledby="landing-start-title">
    <div>
      <p class="landing-kicker">Start in a few lines</p>
      <h2 id="landing-start-title">Bring your provider. Keep your project.</h2>
      <p>Install the release binary on macOS or Linux, set a provider key, and launch niminal from the workspace you want to work on.</p>
    </div>
    <pre><code><span class="landing-prompt">$</span> curl -fsSL https://niminal.dev/install.sh | sh
<span class="landing-prompt">$</span> export OPENROUTER_API_KEY=your-key
<span class="landing-prompt">$</span> cd /path/to/your/project
<span class="landing-prompt">$</span> niminal</code></pre>
  </section>

  <p class="landing-footer-link"><a href="/guides/install/">Install niminal</a>, <a href="/guides/quickstart/">read the quickstart</a>, or <a href="https://github.com/martineastwood/niminal">view niminal on GitHub</a>.</p>
</div>
