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
- ✅ **Cluster-Fokus über Vorlagen (Mod+R)**: will man ein Cluster richtig benutzen, ordnet man es per Vorlage so an, dass es genau einen Bildschirm füllt (wie Snap-Layouts in Windows 11). Mod+R öffnet ein Menü in der Bildschirmmitte mit kleinen Bildern der Vorlagen: ganzer Bildschirm, Hälfte|Hälfte, zwei Drittel|Drittel, Hälfte|zwei Viertel, zwei Viertel|Hälfte, drei Spalten, Viertel|Hälfte|Viertel (für Ultrawide), vier Viertel.
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
- ~~**Einstellung `match_size`**~~ (entfernt 21.11.2026, ersetzt durch Punkt 5b): Standard aus = Fenster behalten beim Andocken ihre Größe (unterschiedlich große Fenster dürfen aneinander docken). An = ein einzeln angedocktes Fenster übernimmt die Höhe (nebeneinander) bzw. Breite (untereinander) des Nachbarn, das Cluster wird ein sauberes Raster. In der Lua-Config: `desk.snapping { match_size = true }`.
  - ✅ Ein Platz, an dem schon ein Fenster liegt, wird nicht angedockt (keine überlappenden Fenster).
  - ✅ **Überschießen wird verziehen:** Schiebt man ein Fenster zu weit, schon über den Nachbarn, dockt es trotzdem an dessen Kante an (bis 60 px tief, Bildschirm). Beim Heranziehen bleibt es bei 15 px.
  - ✅ Schnell vorbeiziehen (über 1000 px/s) rastet nicht ein, damit nichts kurz hin- und herspringt. Beim Loslassen wird immer eingerastet.
- ✅ Zieht man ein Fenster, bewegt sich sein **ganzes Cluster**. Lässt man los und das Cluster berührt andere Fenster, werden sie zu einem Cluster verbunden.
- ✅ Nur Ecke-an-Ecke zählt nicht als angedockt, es braucht eine gemeinsame Kante.
- ✅ Resize einer geteilten Kante (eingebaut: **Alt + Rechtsziehen**, es bewegen sich die Kanten nächst zum Mauszeiger; auch Ziehen am Fensterrand des Programms), zwei Modi:
  - ✅ **Am Fensterrand ziehen** (ohne Alt): eigene Greifzone, 10 px außerhalb und 5 px innerhalb des Fensters (Bildschirm-Pixel). Rückmeldung: Resize-Pfeile als Mauszeiger, und die Kanten, die sich bewegen werden (auch die der Nachbarn), leuchten blau.
  - ~~**Kanten rasten beim Resize ein**~~ (entfernt 21.11.2026 mit Punkt 5b, Schritt 2) (wie in PowerPoint): Die gezogene Kante rastet innerhalb von 15 px (Bildschirm) bündig an Kanten anderer sichtbarer Fenster ein, z.B. Unterkante an Unterkante.
  - **Standard:** Nachbarn im Cluster wachsen/schrumpfen mit (der Viewport ist nur ein Bildschirm, daher nicht einfach wegschieben).
  - **Mit Shift (o. ä.) gehalten:** nur das gezogene Fenster ändert seine Größe, das Nachbarfenster (und alles dahinter) wird verschoben.
  - ✅ "Verschoben" gilt für alles, was auf der anderen Seite der Kante im Cluster hängt. Das Cluster bleibt zusammen, wird nur größer/kleiner.
- ✅ Fenster aus dem Cluster lösen: **Alt+Klick, 0,3 s stillhalten (blauer Puls-Ring), dann ziehen** = nur dieses Fenster. Sofort ziehen = ganzes Cluster.
  - Ausprobiert und verworfen: Ruck (zu empfindlich), Schütteln, gegen Widerstand ziehen (ging, aber schlechter). Doppelklick + Ziehen übersprungen.
- ✅ **Mehrere Fenster gemeinsam lösen:** Fenster markieren (blauer Rahmen) mit Alt+Shift+Klick oder per Auswahlrahmen (Alt+Shift+Ziehen auf leerer Fläche). Halten + Ziehen auf ein markiertes Fenster löst alle markierten Fenster dieses Clusters zusammen heraus; sie bleiben untereinander verbunden. Klick auf leere Fläche hebt die Markierung auf.
- ✅ Orangener Punkt in der Skizze: ohne Bedeutung (Artefakt vom Zeichenprogramm).

## 5b. Feste Fenstergrößen im 2×2-Raster (entschieden 21.11.2026)

Ersetzt die freien Fenstergrößen aus Punkt 5 (`match_size`, freies Resize, Ausrichten unterschiedlich großer Fenster). Ziel: Fenster sitzen immer genau im Bildschirm, nichts ragt angeschnitten heraus.

- ✅ Ein Bildschirm ist ein 2×2-Raster. Breite und Höhe eines Fensters sind je **halb oder ganz** (für jeden Monitor berechnet): Viertel, linke/rechte Hälfte, obere/untere Hälfte, ganzer Bildschirm.
- ✅ **Lücken** zwischen den Fenstern und zum Bildschirmrand wie in niri, einstellbar (`desk.layout { gaps = 16 }`). Die Größen ziehen die Lücken ab, sodass 2×2 Fenster mit Lücken genau einen Bildschirm füllen.
- ✅ **Freie Cluster:** Cluster schweben weiter frei auf der Fläche, innerhalb eines Clusters liegen die Fenster im Raster. Bewusst **kein** großes Raster über die ganze Fläche: die Freiheit, Cluster überall hinzulegen, bleibt (Pyro).
- ✅ **Ansicht rastet ein, wenn sie schon fast passt** (wie das Andocken, nur für die "Kamera"): Beim Loslassen (Maus, Touchpad), nur bei 1:1, rutscht sie das letzte Stück, wenn ein Cluster schon beinahe genau im Bildschirm sitzt (Stellen in halben Bildschirm-Schritten des Clusters, höchstens etwa ein Zehntel des Bildschirms entfernt). Weiter weg bleibt sie, wo sie losgelassen wurde; herausgezoomt immer frei. Verworfen: immer einrasten (nimmt die Freiheit).
- ✅ Größe ändern (Rand ziehen, Mod+Shift+Pfeile) wechselt zwischen halb und ganz.
- ✅ Vorlagen (Mod+R) nur noch die, die ins 2×2-Raster passen; Drittel-Vorlagen fallen weg.
- ✅ Dialoge bleiben frei schwebend in der Mitte.
- ✅ Schritt 1 eingebaut (21.11.2026): Lücken (Standard 16 px), neue Fenster halbe Breite × ganze Höhe, frei in der linken Bildschirmhälfte, Andocken und Vorlagen mit Lücke, `match_size` entfernt. Die Größe wird beim Öffnen aus dem Bildschirm berechnet (ändert sich der Bildschirm danach, behalten offene Fenster ihre Größe). Noch frei: Größe ändern (Schritt 2), Ansicht einrasten (Schritt 3).
- ✅ Schritt 2 eingebaut (21.11.2026): Größe ändern springt zwischen halb und ganz, sobald die Kante über die halbe Strecke gezogen ist (Mod+Shift+Pfeil: rechts/unten macht halb zu ganz, links/oben ganz zu halb). **Nachbarn geben Platz ab oder nehmen ihn, wenn sie dabei zwischen halb und ganz bleiben; sonst rücken sie samt allem dahinter zur Seite** (wie in niri; Pyro einverstanden). Mit Shift rückt wie bisher immer alles. Geht beides nicht (z.B. Mindestgröße eines Programms), passiert nichts. Das Einrasten der gezogenen Kante an fremden Kanten ist weg (bringt mit festen Größen nichts). Vorlagen: ganzer Bildschirm, Hälfte | Hälfte, Hälfte | zwei Viertel, zwei Viertel | Hälfte, vier Viertel (Tasten 1–5, eine Reihe im Menü).

## 6. Navigation

- ✅ Pan **und** Zoom, damit man sich schnell einen Überblick über die ganze Fläche verschaffen kann.
- ✅ **Zoom-Grenze rein:** maximal 1:1 (Zoomfaktor 1.0, Fenster in ihrer normalen Pixelgröße). Man kann also nie in ein Fenster "hineinzoomen". Das passt sich an die Display-Auflösung an (bei 1920×1080 ist 1:1 die Grenze), damit man nicht auf wenige Pixel eines Fensters zoomen kann.
- ✅ **Zoom-Grenze raus:** so weit, dass die ganze Canvas-Fläche auf den Bildschirm passt.
- ✅ **Tastatur-Bedienung** (17.11.2026, von Pyro ausgewählt; Mod = Alt genestet, später Super; alles per `desk.bind` änderbar). Keine Workspaces: InfinityDesk hat ein eigenes Konzept und muss sich nicht an andere Fenstermanager halten.
  - Mod+Pfeile: Fokus zum nächsten Fenster in der Richtung, auch ins nächste Cluster; die Ansicht rückt mit.
  - Mod+Strg+Pfeile: aktives Fenster mit dem Nachbarn im Cluster tauschen (Plätze tauschen, Größen gehen mit).
  - Mod+Shift+Pfeile: Größe ändern, seit Schritt 2 von Punkt 5b zwischen halb und ganz (vorher in Schritten von 10 % des Bildschirms). Es bewegt sich immer nur die rechte bzw. untere Kante, und zwar in Pfeilrichtung: → und ↓ machen größer, ← und ↑ kleiner (Pyro, nach dem ersten Test; vorher wanderte je nach Nachbar mal die linke/obere Kante, und ↑ hieß größer). Nachbarn gehen mit wie beim Ziehen.
  - Mod+Strg+Shift+Pfeile: Ansicht verschieben · Mod+Minus/Plus: zoomen um die Bildschirmmitte · Mod+C: aktives Cluster in die Mitte.
  - Mod+Tab und Mod+O: Übersicht (alle Fenster passen auf den Bildschirm; Mod+O, weil niri Alt+Tab genestet für sich behält). Nochmal Mod+Tab: zurück, oder zum Fenster, das in der Übersicht gewählt wurde (Klick oder Pfeile).
  - Alt+Tab: zuletzt benutzte Fenster. Alt halten, jedes Tab geht eins weiter zurück, die Ansicht fährt hin (die Fläche ist die Vorschau); Loslassen wählt. Genestet fängt niri Alt+Tab ab (und Mod+Tab wären dieselben Tasten); zum Ausprobieren `desk.bind("Ctrl+Tab", desk.recent_windows)`. 💬 Eigene Vorschau-Leiste nur, wenn sie beim Ausprobieren fehlt.
  - Mod+R: Vorlagen-Menü (war Mod+W; Super+W ist bei Pyro Firefox) · Mod+Leertaste: Launcher (war Mod+D).
- ✅ **Touchpad-Gesten** (18.11.2026, von Pyro ausgewählt; gelten überall, auch über Fenstern, Programme bekommen sie nicht; aus bei Ziehen und Vollbild):
  - Pinch: zoomen um den Mauszeiger.
  - Drei Finger wischen: Ansicht verschieben, der Inhalt folgt den Fingern.
  - Vier Finger hoch: Übersicht · runter: zurück (wie niri).
  - Scrollen (zwei Finger oder Mausrad) auf leerer Fläche: Ansicht verschieben. Über Fenstern scrollt es im Programm. Eine Mausrad-Raste verschiebt 60 px.
  - Genestet behält niri die Drei- und Vier-Finger-Gesten für sich; zum Ausprobieren die eigenständige Sitzung.
- 💬 Maus-Belegung und Gesten in der Config – später.
- Vorläufige Test-Bedienung (Platzhalter, bis die Bedienung entschieden ist):
  - Linksziehen auf leerer Fläche: Fläche verschieben (Pan)
  - Alt + Mausrad: zoomen (um den Mauszeiger herum)
  - Alt + Linksziehen auf einem Fenster: Cluster verschieben
  - Alt + Rechtsziehen: Größe ändern, Nachbarn im Cluster passen sich an · mit Shift: Rest des Clusters rückt mit
  - Alt + Linksklick, kurz stillhalten, dann ziehen: Fenster aus dem Cluster lösen (markierte Fenster gemeinsam)
  - Alt + Shift + Klick: Fenster markieren · Alt + Shift + Ziehen auf leerer Fläche: Auswahlrahmen
  - Tastatur: siehe oben. Dazu Mod+F Vollbild, Mod+T und Mod+Enter Terminal (kitty, vorher alacritty), Mod+Q Fenster schließen (wie niri). Mod = Alt im genesteten Betrieb, später Super
  - (Super ist im genesteten Betrieb von niri belegt, daher Alt. In der eigenständigen Sitzung werden alle Alt-Kürzel zu Super-Kürzeln, so will es Pyro.)

## 6b. Hintergrund

- 💡 Ebenen von hinten nach vorne: **Hintergrund** (fest am Bildschirm) → **Canvas** (Raster + Fenster, bewegt sich beim Pan) → ggf. Leisten obendrauf.
- ✅ `layer-shell`-Protokoll ist eingebaut (für fuzzel). Damit sollten auch swaybg/swww (Wallpaper) und Waybar (Leiste) funktionieren.
- ✅ Hintergrund ist **fest** (Pyro, 17.11.2026). Das macht ein Wallpaper-Programm wie `awww` oder `swaybg` über layer-shell; InfinityDesk muss dafür nichts zeichnen. (awww brauchte das Protokoll `wp_viewporter`, ist eingebaut.) Parallax verworfen.

## 6c. Veröffentlichung & Konfiguration

- ✅ Das Repo wird veröffentlicht; andere sollen InfinityDesk nutzen und anpassen können.
- ✅ Konfiguration per **Lua** (z. B. `~/.config/infinitydesk/init.lua`), über die Rust-Bibliothek `mlua`.
- 💡 Ausbaustufen:
  1. ✅ **Einstellungen** (eingebaut): Tastatur (xkb-Belegung, Wiederholrate), Canvas-Größe, Raster-Abstand, Snap-Schwellwert, Lücken (`desk.layout { gaps = 16 }`), Haltezeit der Lös-Geste, Farben (Hintergrund, Raster, Akzent). Gelesen beim Start und **neu geladen, sobald die Datei gespeichert wird** (einmal pro Sekunde wird nachgesehen); Tippfehler, falsche Typen und Werte außerhalb des Bereichs sind Fehler (mit Einstellung, Datei und Zeile im Log), dann gelten beim Start die Standardwerte, im laufenden Betrieb bleiben die bisherigen. Wird die Fläche kleiner, rücken Cluster außerhalb als Ganzes herein. Lua 5.4 ist in die Binary eingebaut (`mlua`, vendored). `INFINITYDESK_CONFIG` zeigt auf eine andere Datei (zum Testen). Touchpad (`desk.input { touchpad = { tap, natural_scroll, dwt, accel_speed, accel_profile } }`, nur eigenständig; Standard wie bei Pyro in niri: Tippen, natürliches Scrollen, aus beim Tippen, Beschleunigung 0). Noch nicht: Maus-Einstellungen, Zoom-Verhalten.
  2. ✅ **Tastenbelegung** (eingebaut): `desk.bind("Mod+Return", desk.spawn("alacritty"))`, `desk.unbind("Mod+Tab")`. Die eingebauten Kürzel (siehe Punkt 6) bleiben, eigene gehen vor oder kommen dazu. Modifikatoren Mod/Super/Alt/Ctrl/Shift, Tasten als xkb-Namen; "Shift+Q" meint die Q-Taste. Aktionen: `desk.spawn("Befehlszeile")` (über `sh -c`, also mit Argumenten), `desk.close_window`, `desk.fullscreen`, `desk.layout_menu`, `desk.focus/swap/resize/pan("left"|"right"|"up"|"down")`, `desk.zoom_in`, `desk.zoom_out`, `desk.center`, `desk.overview`, `desk.recent_windows`. Lua-Funktionen als Aktion erst mit Stufe 4.
     **Maus-Bedienung:** wird belegbar, wenn die endgültige Bedienung feststeht (Punkt 6).
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
  desk.layout { gaps = 16 }
  desk.snapping { threshold = 15, hold_ms = 300 }

  desk.bind("Mod+Return", desk.spawn("kitty"))
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
4. ✅ Zoom + Culling · ✅ Cluster-Resize · ✅ Cluster-Fokus (Vorlagen, Mod+R) · ✅ Vollbild (Mod+F)
5. ✅ Lua-Konfiguration (Einstellungen, Tastenbelegung; Maus-Belegung mit Punkt 6)
6. Bedienung festlegen (Standard-Belegung), Hintergrund / layer-shell, Feinschliff
7. Standalone-Betrieb (eigener Login-Eintrag, ohne niri darunter), README, Veröffentlichung

## 7b. Eigenständige Sitzung (begonnen 17.11.2026)

- ✅ Start ohne `WAYLAND_DISPLAY`/`DISPLAY` (z. B. auf einer Text-Konsole) = eigenständig, sonst genestet als Fenster. Eigenständig ist **Mod = Super**, auch für die Maus-Gesten (Mod+Ziehen, Mod+Mausrad).
- ✅ Erste Stufe, so klein wie möglich: **eine Grafikkarte, ein Monitor** (die erste Karte mit angeschlossenem Monitor; bei Pyro die NVIDIA RTX 4070 SUPER mit DP-2), Auflösung des Monitors mit der höchsten Bildrate dort (bei Pyro 3440×1440 @ 175 Hz).
- ✅ Mauszeiger zeichnet InfinityDesk selbst (Thema aus `XCURSOR_THEME`/`XCURSOR_SIZE`), auch die Zeiger der Programme (z. B. Text-Cursor in Firefox).
- ✅ dmabuf: Programme geben ihre Bilder direkt als GPU-Puffer weiter.
- ✅ Mod+Shift+E beendet InfinityDesk (wie niri). Strg+Alt+F1…F12 wechselt die Konsole.
- ✅ Login-Eintrag `resources/infinitydesk.desktop` (für den Login-Manager, wenn die Konsole klappt).
- Noch nicht: mehrere Monitore, Monitor-Einstellungen in der Config, XWayland (Steam, ältere Programme; z. B. über xwayland-satellite wie niri), Programme beim Start (Autostart), Maus-Einstellungen (Beschleunigung: bei Pyro flach, 0.45), Zeiger-Thema in der Config, Monitor ab-/anstecken im Betrieb.

## 8. Offene Fragen

- Bedienung des Ganzen (Punkt 6)
- Mehrere Monitore: ein Canvas pro Monitor oder ein gemeinsamer?
- ✅ Neue Fenster (eingebaut): zuerst füllen sie freie Plätze der letzten Vorlage (Mod+R). Sonst docken sie am **aktiven Fenster** an: rechts daneben (Oberkanten bündig), sonst darunter (linke Kanten bündig), links, oben. Seiten, die auf dem Bildschirm liegen, kommen zuerst; ist nirgends auf dem Bildschirm Platz, dockt es trotzdem an der ersten freien Seite an und die **Ansicht rückt gerade so weit mit**, dass es ganz zu sehen ist. Ist kein Fenster aktiv (vorher auf leere Fläche geklickt), kommt es in die linke Bildschirmhälfte (seit 21.11.2026, vorher frei in der Mitte). Neue Fenster sind halb so breit und so hoch wie der Bildschirm (mit Lücken), zwei neue Fenster füllen ihn also genau. Dialoge erscheinen immer frei in der Mitte und wählen ihre Größe selbst. Ein Launcher (fuzzel) ändert das aktive Fenster nicht. 💬 Feinabstimmung beim Ausprobieren.
