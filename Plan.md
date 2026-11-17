# InfinityDesk – Plan

Lebendes Dokument, das wir gemeinsam schreiben. Status je Punkt: ✅ entschieden · 💬 offen · 💡 Idee

## 1. Vision

Ein eigener Wayland-Compositor / Window Manager, inspiriert von niri (Freiheit in vertikal + horizontal), aber mit
einer **großen, begrenzten 2D-Fläche**, auf der Fenster frei platziert werden können.
Fenster lassen sich wie bei Windows Snap **aneinander andocken** und bilden so **Cluster**, die auf der Fläche "schweben".

Skizze: Mehrere Cluster (2er-Gruppen, 2×2-Block, Einzelfenster) verteilt auf einer großen Fläche; blaue Punkte = Ecken/Andockpunkte der Fenster.

## Arbeitsweise

- **Pyro:** Design, Verhalten, Bedienung, Entscheidungen, Anleitung. Rust-Vorkenntnisse minimal, Smithay neu.
- **Claude:** schreibt den Code, entscheidet Architektur im Detail, erklärt kurz was und warum, hält den Code lesbar.
- Kleine, lauffähige Schritte: nach jedem Schritt etwas, das man ausprobieren und beurteilen kann.

## 2. Kernkonzepte

- ✅ **Canvas**: sehr große, aber begrenzte Fläche (nicht wirklich unendlich). Größe: 💬 (Vorschlag: ~20000×12000 logische px, konfigurierbar)
- ✅ **Viewport**: der Monitor zeigt einen Ausschnitt des Canvas.
- ✅ **Fenster**: frei platzier- und skalierbar.
- ✅ **Cluster**: Gruppe aneinander angedockter Fenster, als Ganzes verschiebbar.
- ✅ **Freie Ansicht**: normalerweise betrachtet man die Fläche frei aus jeder Position, nichts rastet ein.
- ✅ **Cluster-Fokus (Viewport andocken)**: will man ein Cluster richtig benutzen, wird der Viewport so daran angedockt, dass die linke obere Ecke des obersten linken Fensters an der linken oberen Ecke des Bildschirms sitzt. 💬 Soll dabei auch gezoomt werden, damit das ganze Cluster auf den Bildschirm passt?
- 💡 **Culling**: nur sichtbare Fenster rendern, dann kostet die Canvas-Größe praktisch keine Performance.

## 3. Technik

- ✅ **Rust + Smithay** (Smithay = Rust-Bibliothek mit Bausteinen für Wayland-Compositoren; niri nutzt dasselbe, viel Referenzcode).
- Alternative: wlroots (C).
- Entwicklung zunächst **genestet** (Compositor läuft als Fenster in der bestehenden Session, via winit), damit niri nicht kaputtgeht.

## 4. Datenmodell (Entwurf)

- `Canvas { size, viewport }`
- `Window { id, rect (Canvas-Koordinaten), cluster_id }`
- `Cluster`: Graph aus Fenstern und gemeinsamen Kanten (kein starres Raster, damit L-Formen und 2×2 möglich sind)

## 5. Andocken / Snapping

- ✅ Einrasten an Kanten innerhalb von 15 px: **Andocken** (Kante an Kante) und **Ausrichten** (bündige Ober-/Seitenkanten, ergibt saubere Ecken).
- ✅ Zieht man ein Fenster, bewegt sich sein **ganzes Cluster**. Lässt man los und das Cluster berührt andere Fenster, werden sie zu einem Cluster verbunden.
- ✅ Nur Ecke-an-Ecke zählt nicht als angedockt, es braucht eine gemeinsame Kante.
- ✅ Resize einer geteilten Kante, zwei Modi:
  - **Standard:** Nachbarn im Cluster wachsen/schrumpfen mit (der Viewport ist nur ein Bildschirm, daher nicht einfach wegschieben).
  - **Mit Shift (o. ä.) gehalten:** nur das gezogene Fenster ändert seine Größe, das Nachbarfenster (und alles dahinter) wird verschoben.
  - ✅ "Verschoben" gilt für alles, was auf der anderen Seite der Kante im Cluster hängt. Das Cluster bleibt zusammen, wird nur größer/kleiner.
- 💬 Fenster aus dem Cluster lösen, ohne Taste. Der Ruck (schnelles Wegziehen) war zu empfindlich und die anderen Fenster reisten kurz mit. Wir probieren der Reihe nach:
  1. 🧪 **Schütteln** (aktuell): 3 Richtungswechsel à 40 px innerhalb von 0,6 s
  2. Gegen Widerstand ziehen
  3. Doppelklick + Ziehen
  4. Lange drücken (~0,4 s stillhalten, dann allein ziehen)
- ✅ Orangener Punkt in der Skizze: ohne Bedeutung (Artefakt vom Zeichenprogramm).

## 6. Navigation

- ✅ Pan **und** Zoom, damit man sich schnell einen Überblick über die ganze Fläche verschaffen kann.
- ✅ **Zoom-Grenze rein:** maximal 1:1 (Zoomfaktor 1.0, Fenster in ihrer normalen Pixelgröße). Man kann also nie in ein Fenster "hineinzoomen". Das passt sich an die Display-Auflösung an (bei 1920×1080 ist 1:1 die Grenze), damit man nicht auf wenige Pixel eines Fensters zoomen kann.
- ✅ **Zoom-Grenze raus:** so weit, dass die ganze Canvas-Fläche auf den Bildschirm passt.
- 💬 Bedienung (Maus / Tastatur / Touchpad-Gesten) – später.
- Vorläufige Test-Bedienung (Platzhalter, bis die Bedienung entschieden ist):
  - Linksziehen auf leerer Fläche: Fläche verschieben (Pan)
  - Alt + Linksziehen auf einem Fenster: Cluster verschieben
  - Mod+T Terminal (alacritty), Mod+D Launcher (fuzzel), Mod+Q Fenster schließen (wie niri). Mod = Alt im genesteten Betrieb, später Super
  - (Super ist im genesteten Betrieb von niri belegt, daher Alt)

## 6b. Hintergrund (später)

- 💡 Ebenen von hinten nach vorne: **Hintergrund** (fest am Bildschirm) → **Canvas** (Raster + Fenster, bewegt sich beim Pan) → ggf. Leisten obendrauf.
- ✅ `layer-shell`-Protokoll ist eingebaut (für fuzzel). Damit sollten auch swaybg/swww (Wallpaper) und Waybar (Leiste) funktionieren.
- 💬 Hintergrund komplett fest oder leichter **Parallax**-Effekt (bewegt sich beim Pannen langsam mit)?

## 6c. Veröffentlichung & Konfiguration

- ✅ Das Repo wird veröffentlicht; andere sollen InfinityDesk nutzen und anpassen können.
- ✅ Konfiguration per **Lua** (z. B. `~/.config/infinitydesk/init.lua`), über die Rust-Bibliothek `mlua`.
- 💡 Ausbaustufen:
  1. **Einstellungen:** Canvas-Größe, Snap-Schwellwert, Werte der Lös-Geste, Farben, Raster, Zoom-Verhalten …
  2. **Tastenbelegung & Maus-Bedienung:** frei belegbar, mit sinnvollen Standardwerten.
  3. **Fensterregeln:** z. B. "Programm X öffnet immer an Position Y / in Cluster Z".
  4. **Ereignisse & Skripting:** Lua-Funktionen, die auf Ereignisse reagieren (Fenster geöffnet, Cluster gebildet …).
- ✅ **Leichter Umstieg von niri/Hyprland:** gleiche Begriffe, Struktur und (wo sinnvoll) gleiche Standardtasten wie dort, nur in Lua.
- 💡 Zielbild (Entwurf, Namen noch nicht final):

  ```lua
  -- ~/.config/infinitydesk/init.lua
  local desk = require("infinitydesk")

  desk.input {
    keyboard = { xkb = { layout = "de", options = "caps:escape" }, repeat_delay = 200, repeat_rate = 25 },
    touchpad = { tap = true, natural_scroll = true },
  }

  desk.canvas { width = 20000, height = 12000, grid = { spacing = 100 } }
  desk.snapping { threshold = 15, ruck = { distance = 120, time_ms = 150 } }

  desk.bind("Mod+Return", desk.spawn("alacritty"))
  desk.bind("Mod+Q", desk.close_window)

  desk.window_rule { match = { app_id = "firefox" }, open_at = { x = 1000, y = 500 } }

  desk.on("window_opened", function(window) --[[ eigene Logik ]] end)
  ```
- 💡 Später evtl. ein Konverter, der eine niri-Config (KDL) einliest und eine `init.lua` erzeugt.
- 💡 Referenz: *Pinnacle* ist ebenfalls ein Smithay-Compositor mit Lua-Konfiguration.
- 💡 Neu geladen werden ohne Neustart (Hot-Reload), wenn möglich.
- ✅ Code, Kommentare, Logs, README und Doku auf **Englisch** (öffentliches Repo). Dieser Plan bleibt unser deutsches Arbeitsdokument.

## 7. Roadmap

1. ✅ Minimaler Compositor (genestet, winit), zeigt Wayland-Clients an
2. ✅ Fenster auf dem Canvas platzieren, Viewport verschieben
3. ✅ Snapping + Cluster
4. Cluster-Resize, Culling, Zoom, Cluster-Fokus
5. Lua-Konfiguration (Einstellungen, dann Tastenbelegung)
6. Bedienung festlegen (Standard-Belegung), Hintergrund / layer-shell, Feinschliff
7. Standalone-Betrieb (eigener Login-Eintrag, ohne niri darunter), README, Veröffentlichung

## 8. Offene Fragen

- Bedienung des Ganzen (Punkt 6)
- Mehrere Monitore: ein Canvas pro Monitor oder ein gemeinsamer?
- ✅ Neue Fenster: ist **kein Cluster fokussiert**, erscheint das Fenster in der Mitte des Bildschirms. Ist ein **Cluster fokussiert**, dockt es neben den Fenstern an, die schon im Cluster sind. Stelle: **rechts neben dem zuletzt fokussierten Fenster**; ist dort kein Platz, die nächste freie Seite. Muss im Viewport sichtbar bleiben. 💬 Feinabstimmung beim Ausprobieren.
