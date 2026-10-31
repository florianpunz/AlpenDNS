// Vor dem ersten Zeichnen, und deshalb im Kopf statt in app.js: stünde die Wahl
// erst am Ende des Dokuments, blitzte bei jedem Laden kurz das helle Thema auf.
// Ohne eigene Wahl gilt die des Systems.
try {
  var saved = localStorage.getItem("alpendns.theme");
  if (saved !== "dark" && saved !== "light") {
    saved = matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  }
  document.documentElement.dataset.theme = saved;
} catch (e) {
  // Ohne Speicher bleibt es beim hellen Thema, das :root festlegt.
}
