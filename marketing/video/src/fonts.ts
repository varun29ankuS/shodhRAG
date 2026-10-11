// Fonts are bundled from node_modules (no network at render time) and the
// first frame waits until they are ready.
import "@fontsource-variable/geist";
import "@fontsource/noto-sans-devanagari/devanagari-500.css";
import { cancelRender, continueRender, delayRender } from "remotion";

const handle = delayRender("Loading fonts");
Promise.all([
  document.fonts.load('450 36px "Geist Variable"'),
  document.fonts.load('550 64px "Geist Variable"'),
  document.fonts.load('500 54px "Noto Sans Devanagari"', "शोध"),
])
  .then(() => continueRender(handle))
  .catch((err: unknown) => cancelRender(err));
