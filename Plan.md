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
- ✅ **Cluster-Fokus über Vorlagen (Mod+W)**: will man ein Cluster richtig benutzen, ordnet man es per Vorlage so an, dass es genau einen Bildschirm füllt (wie Snap-Layouts in Windows 11). Mod+W öffnet ein Menü in der Bildschirmmitte mit kleinen Bildern der Vorlagen: ganzer Bildschirm, Hälfte|Hälfte, zwei Drittel|Drittel, Hälfte|zwei Viertel, zwei Viertel|Hälfte, drei Spalten, Viertel|Hälfte|Viertel (für Ultrawide), vier Viertel.
  - Klick auf einen Platz = das aktive Fenster kommt dorthin, die übrigen Fenster des Clusters in die Plätze, die ihrer jetzigen Lage am nächsten sind. Tasten 1–8 = Vorlage mit ganz automatischer Zuordnung. Esc oder Klick daneben schließt.
  - Vorlagen mit weniger Plätzen als das Cluster Fenster hat sind ausgegraut.
  - Freie Plätze bleiben frei; die nächsten neuen Fenster füllen sie der Reihe nach (nur wenn der Platz gerade auf dem Bildschirm und nicht von einem anderen Fenster belegt ist).
  - Die Vorlage beginnt an der linken oberen Ecke des Clusters; die Ansicht springt hin (1:1). Danach bleibt die Ansicht frei (kein gesperrter Modus).
  - Verworfen: Mod+F springt nur mit dem Cluster in die Bildschirmecke (zu wenig: man will das Cluster gleich passend anordnen).
- ✅ **Vollbild (Mod+F)**: das aktive Fenster liegt bildschirmfüllend über allem, auch über Leisten. Sein Platz im Cluster und die Fläche darunter bleiben, Schieben/Zoomen/Ränder sind aus. Mod+F nochmal = zurück an den alten Platz und zur alten Ansicht. Programme können es auch selbst anfordern (F11, Video-Vollbild). Ein neues Fenster beendet das Vollbild.
- ✅ **Culling**: nur sichtbare Fenster werden gerendert (eingebaut mit dem Zoom).

## 3. Technik

- ✅ **Rust + Smithay** (Smithay = Rust-Bibliothek mit Bausteinen für Wayland-Compositoren; niri nutzt dasselbe, viel Referenzcode).
- Alternative: wlroots (C).
- Entwicklung zunächst **genestet** (Compositor läuft als Fenster in der bestehenden Session, via winit), damit niri nicht kaputtgeht.

## 4. Datenmodell (Entwurf)

- `Canvas { size, viewport }`
- `Window { id, rect (Canvas-Koordinaten), cluster_id }`
- `Cluster`: Graph aus Fenstern und gemeinsamen Kanten (kein starres Raster, damit L-Formen und 2×2 möglich sind)

## 5. Andocken / Snapping

- ✅ Einrasten an Kanten innerhalb von 15 px **auf dem Bildschirm** (gilt bei jedem Zoom gleich): **Andocken** (Kante an Kante) und **Ausrichten** (bündige Ober-/Seitenkanten, ergibt saubere Ecken).
- ✅ **Keine versetzten Verbindungen:** Beim Andocken richtet sich das Fenster immer bündig am Nachbarn aus (nebeneinander: Ober- **oder** Unterkanten, untereinander: linke **oder** rechte Kanten, je nachdem welches Paar näher ist). So docken auch unterschiedlich große Fenster an und es entstehen saubere Ecken. Angedockt wird nur, wenn diese Kanten schon nah sind (höchstens 60 px auf dem Bildschirm), sonst würde das Fenster weit springen.
- ✅ **Einstellung `match_size`:** Standard aus = Fenster behalten beim Andocken ihre Größe (unterschiedlich große Fenster dürfen aneinander docken). An = ein einzeln angedocktes Fenster übernimmt die Höhe (nebeneinander) bzw. Breite (untereinander) des Nachbarn, das Cluster wird ein sauberes Raster. Bis zur Lua-Config: `INFINITYDESK_MATCH_SIZE=1 cargo run`.
  - ✅ Ein Platz, an dem schon ein Fenster liegt, wird nicht angedockt (keine überlappenden Fenster).
  - ✅ **Überschießen wird verziehen:** Schiebt man ein Fenster zu weit, schon über den Nachbarn, dockt es trotzdem an dessen Kante an (bis 60 px tief, Bildschirm). Beim Heranziehen bleibt es bei 15 px.
  - ✅ Schnell vorbeiziehen (über 1000 px/s) rastet nicht ein, damit nichts kurz hin- und herspringt. Beim Loslassen wird immer eingerastet.
- ✅ Zieht man ein Fenster, bewegt sich sein **ganzes Cluster**. Lässt man los und das Cluster berührt andere Fenster, werden sie zu einem Cluster verbunden.
- ✅ Nur Ecke-an-Ecke zählt nicht als angedockt, es braucht eine gemeinsame Kante.
- ✅ Resize einer geteilten Kante (eingebaut: **Alt + Rechtsziehen**, es bewegen sich die Kanten nächst zum Mauszeiger; auch Ziehen am Fensterrand des Programms), zwei Modi:
  - ✅ **Am Fensterrand ziehen** (ohne Alt): eigene Greifzone, 10 px außerhalb und 5 px innerhalb des Fensters (Bildschirm-Pixel). Rückmeldung: Resize-Pfeile als Mauszeiger, und die Kanten, die sich bewegen werden (auch die der Nachbarn), leuchten blau.
  - ✅ **Kanten rasten beim Resize ein** (wie in PowerPoint): Die gezogene Kante rastet innerhalb von 15 px (Bildschirm) bündig an Kanten anderer sichtbarer Fenster ein, z.B. Unterkante an Unterkante.
  - **Standard:** Nachbarn im Cluster wachsen/schrumpfen mit (der Viewport ist nur ein Bildschirm, daher nicht einfach wegschieben).
  - **Mit Shift (o. ä.) gehalten:** nur das gezogene Fenster ändert seine Größe, das Nachbarfenster (und alles dahinter) wird verschoben.
  - ✅ "Verschoben" gilt für alles, was auf der anderen Seite der Kante im Cluster hängt. Das Cluster bleibt zusammen, wird nur größer/kleiner.
- ✅ Fenster aus dem Cluster lösen: **Alt+Klick, 0,3 s stillhalten (blauer Puls-Ring), dann ziehen** = nur dieses Fenster. Sofort ziehen = ganzes Cluster.
  - Ausprobiert und verworfen: Ruck (zu empfindlich), Schütteln, gegen Widerstand ziehen (ging, aber schlechter). Doppelklick + Ziehen übersprungen.
- ✅ **Mehrere Fenster gemeinsam lösen:** Fenster markieren (blauer Rahmen) mit Alt+Shift+Klick oder per Auswahlrahmen (Alt+Shift+Ziehen auf leerer Fläche). Halten + Ziehen auf ein markiertes Fenster löst alle markierten Fenster dieses Clusters zusammen heraus; sie bleiben untereinander verbunden. Klick auf leere Fläche hebt die Markierung auf.
- ✅ Orangener Punkt in der Skizze: ohne Bedeutung (Artefakt vom Zeichenprogramm).

## 6. Navigation

- ✅ Pan **und** Zoom, damit man sich schnell einen Überblick über die ganze Fläche verschaffen kann.
- ✅ **Zoom-Grenze rein:** maximal 1:1 (Zoomfaktor 1.0, Fenster in ihrer normalen Pixelgröße). Man kann also nie in ein Fenster "hineinzoomen". Das passt sich an die Display-Auflösung an (bei 1920×1080 ist 1:1 die Grenze), damit man nicht auf wenige Pixel eines Fensters zoomen kann.
- ✅ **Zoom-Grenze raus:** so weit, dass die ganze Canvas-Fläche auf den Bildschirm passt.
- 💬 Bedienung (Maus / Tastatur / Touchpad-Gesten) – später.
- Vorläufige Test-Bedienung (Platzhalter, bis die Bedienung entschieden ist):
  - Linksziehen auf leerer Fläche: Fläche verschieben (Pan)
  - Alt + Mausrad: zoomen (um den Mauszeiger herum)
  - Alt + Linksziehen auf einem Fenster: Cluster verschieben
  - Alt + Rechtsziehen: Größe ändern, Nachbarn im Cluster passen sich an · mit Shift: Rest des Clusters rückt mit
  - Alt + Linksklick, kurz stillhalten, dann ziehen: Fenster aus dem Cluster lösen (markierte Fenster gemeinsam)
  - Alt + Shift + Klick: Fenster markieren · Alt + Shift + Ziehen auf leerer Fläche: Auswahlrahmen
  - Mod+W Vorlagen-Menü, Mod+F Vollbild
  - Mod+T Terminal (alacritty), Mod+D Launcher (fuzzel), Mod+Q Fenster schließen (wie niri). Mod = Alt im genesteten Betrieb, später Super
  - (Super ist im genesteten Betrieb von niri belegt, daher Alt. In der eigenständigen Sitzung werden alle Alt-Kürzel zu Super-Kürzeln, so will es Pyro.)

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
  desk.snapping { threshold = 15, match_size = false, hold_ms = 300 }

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
4. ✅ Zoom + Culling · ✅ Cluster-Resize · ✅ Cluster-Fokus (Vorlagen, Mod+W) · ✅ Vollbild (Mod+F)
5. Lua-Konfiguration (Einstellungen, dann Tastenbelegung)
6. Bedienung festlegen (Standard-Belegung), Hintergrund / layer-shell, Feinschliff
7. Standalone-Betrieb (eigener Login-Eintrag, ohne niri darunter), README, Veröffentlichung

## 8. Offene Fragen

- Bedienung des Ganzen (Punkt 6)
- Mehrere Monitore: ein Canvas pro Monitor oder ein gemeinsamer?
- ✅ Neue Fenster (eingebaut): zuerst füllen sie freie Plätze der letzten Vorlage (Mod+W). Sonst docken sie am **aktiven Fenster** an: rechts daneben (Oberkanten bündig), sonst darunter (linke Kanten bündig), links, oben. Seiten, die auf dem Bildschirm liegen, kommen zuerst; ist nirgends auf dem Bildschirm Platz, dockt es trotzdem an der ersten freien Seite an und die **Ansicht rückt gerade so weit mit**, dass es ganz zu sehen ist. Ist kein Fenster aktiv (vorher auf leere Fläche geklickt), erscheint es frei in der Mitte. Dialoge erscheinen immer frei in der Mitte. Ein Launcher (fuzzel) ändert das aktive Fenster nicht. 💬 Feinabstimmung beim Ausprobieren.
