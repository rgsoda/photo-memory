const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const pane = document.getElementById("pane");
const image = document.getElementById("image");
const caption = document.getElementById("caption");
const hints = document.getElementById("hints");

const HINTS = {
  fit: "<kbd>Z</kbd> 1:1 &nbsp; <kbd>←</kbd><kbd>→</kbd> next &nbsp; <kbd>Esc</kbd> close",
  zoomed: "drag to pan &nbsp; <kbd>Z</kbd> fit &nbsp; <kbd>Esc</kbd> close",
};

let names = [];
let shown = 0;
let zoomed = false;

/** The window is opened with its subject in the query string, then told about
 *  later ones by event, so a second `V` reuses the window instead of stacking. */
function readParams() {
  const params = new URLSearchParams(location.search);
  try {
    names = JSON.parse(params.get("n") ?? "[]");
  } catch {
    names = [];
  }
  shown = Number(params.get("i") ?? 0);
}

function setZoom(on) {
  zoomed = on;
  document.body.classList.toggle("zoomed", on);
  hints.innerHTML = on ? HINTS.zoomed : HINTS.fit;
  if (!on) pane.scrollTo(0, 0);
}

async function show(index) {
  if (!names.length) return;
  shown = (index + names.length) % names.length;
  const name = names[shown];

  let full;
  try {
    full = await invoke("read_image", { name });
  } catch (e) {
    caption.textContent = String(e);
    return;
  }

  image.src = full.url;
  // Reshape the window to the picture before showing it, so stepping through a
  // note's images never leaves the frame the wrong shape.
  invoke("fit_image_window", { width: full.width, height: full.height }).catch(() => {});

  image.alt = name;
  caption.textContent = names.length > 1 ? `${name}  ·  ${shown + 1}/${names.length}` : name;
  setZoom(false);
}

const close = () => invoke("close_image").catch(() => {});

document.addEventListener("keydown", (e) => {
  switch (e.key) {
    case "Escape":
    case "v":
    case "q":
      e.preventDefault();
      close();
      break;
    case "z":
      e.preventDefault();
      setZoom(!zoomed);
      break;
    case "ArrowRight":
      e.preventDefault();
      show(shown + 1);
      break;
    case "ArrowLeft":
      e.preventDefault();
      show(shown - 1);
      break;
  }
});

image.addEventListener("click", () => setZoom(!zoomed));

pane.addEventListener("pointerdown", (e) => {
  if (!zoomed) return;
  const from = { x: e.clientX, y: e.clientY, left: pane.scrollLeft, top: pane.scrollTop };
  const drag = (m) => {
    pane.scrollLeft = from.left - (m.clientX - from.x);
    pane.scrollTop = from.top - (m.clientY - from.y);
  };
  document.addEventListener("pointermove", drag);
  document.addEventListener("pointerup", () => document.removeEventListener("pointermove", drag), {
    once: true,
  });
});

// A second request while the window is already open arrives here.
listen("photomem://show", ({ payload: [next, index] }) => {
  names = next;
  show(index);
});

readParams();
show(shown);
