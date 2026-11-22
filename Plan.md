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
- ✅ Liegt im Bildschirm-Bereich einer Vorlage schon ein anderes Cluster, **rückt es als Ganzes nach rechts bzw. unten weg**, bis es frei liegt (Pyro, 21.11.2026; vorher überdeckten sich die Fenster). Eingebaut: kürzerer Weg von beiden; was ein gerücktes Cluster trifft, rückt mit; danach liegen die Fenster eine Lücke hinter der Kante (also wieder im Raster des nächsten Bildschirms). Dialoge bleiben, wo sie sind. Nie über den Rand der Fläche hinaus.
- ✅ Dialoge bleiben frei schwebend in der Mitte.
- ✅ Schritt 1 eingebaut (21.11.2026): Lücken (Standard 16 px), neue Fenster halbe Breite × ganze Höhe, frei in der linken Bildschirmhälfte, Andocken und Vorlagen mit Lücke, `match_size` entfernt. Die Größe wird beim Öffnen aus dem Bildschirm berechnet (ändert sich der Bildschirm danach, behalten offene Fenster ihre Größe). Noch frei: Größe ändern (Schritt 2), Ansicht einrasten (Schritt 3).
- ✅ Schritt 2 eingebaut (21.11.2026): Größe ändern springt zwischen halb und ganz, sobald die Kante über die halbe Strecke gezogen ist (Mod+Shift+Pfeil: rechts/unten macht halb zu ganz, links/oben ganz zu halb). **Nachbarn geben Platz ab oder nehmen ihn, wenn sie dabei zwischen halb und ganz bleiben; sonst rücken sie samt allem dahinter zur Seite** (wie in niri; Pyro einverstanden). Mit Shift rückt wie bisher immer alles. Geht beides nicht (z.B. Mindestgröße eines Programms), passiert nichts. Das Einrasten der gezogenen Kante an fremden Kanten ist weg (bringt mit festen Größen nichts). Vorlagen: ganzer Bildschirm, Hälfte | Hälfte, Hälfte | zwei Viertel, zwei Viertel | Hälfte, vier Viertel (Tasten 1–5, eine Reihe im Menü).
- ✅ Schritt 3 eingebaut (21.11.2026): Ansicht rastet ein, wenn ein Schwenk endet (Ziehen auf leerer Fläche loslassen, Drei-Finger-Wischen endet, Finger beim Zwei-Finger-Scrollen gehoben; nicht beim Mausrad und nicht bei den Tasten). Nur bei 1:1. Pro Achse die nächste Stelle, an der ein sichtbares Fenster genau im 2×2-Raster sitzt (eine Lücke vom Rand, in halben Bildschirm-Schritten), höchstens ein Zehntel des Bildschirms entfernt; Stellen, an denen das Fenster nicht mehr zu sehen wäre, zählen nicht. Dialoge zählen nicht. Die Ansicht springt (keine Animation); falls das zu hart wirkt, kann sie später gleiten. → Gleitet seit 22.11.2026 (siehe Animationen).

## 6. Navigation

- ✅ Pan **und** Zoom, damit man sich schnell einen Überblick über die ganze Fläche verschaffen kann.
- ✅ **Zoom-Grenze rein:** maximal 1:1 (Zoomfaktor 1.0, Fenster in ihrer normalen Pixelgröße). Man kann also nie in ein Fenster "hineinzoomen". Das passt sich an die Display-Auflösung an (bei 1920×1080 ist 1:1 die Grenze), damit man nicht auf wenige Pixel eines Fensters zoomen kann.
- ✅ **Zoom-Grenze raus:** so weit, dass die ganze Canvas-Fläche auf den Bildschirm passt.
- ✅ **Tastatur-Bedienung** (17.11.2026, von Pyro ausgewählt; Mod = Alt genestet, später Super; alles per `desk.bind` änderbar). ✅ Was Mod ist, ist einstellbar (22.11.2026, wie niri: `desk.input { mod_key = "Super", mod_key_nested = "Alt" }`, auch "Ctrl"). "Mod" bleibt in den Kürzeln stehen und wird erst beim Tastendruck aufgelöst; passen zwei Belegungen, gewinnt die spätere (die eigene Config vor den eingebauten). Keine Workspaces: InfinityDesk hat ein eigenes Konzept und muss sich nicht an andere Fenstermanager halten.
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
- ✅ **Maus-Bedienung ist final** (Pyro, 21.11.2026) und ✅ in der Config belegbar (22.11.2026, über `desk.bind` wie Tasten, Namen wie in niri: `desk.bind("Mod+MouseRight", desk.drag_resize)`; Aktionen `drag_move`, `drag_resize`, `drag_select` für Maustasten und `scroll_zoom` für `Scroll`. Ohne Modifikator wirken sie nur auf leerer Fläche, über Fenstern bekommt das Programm den Klick. Fest bleiben: Greifzone am Rand, Scrollen auf leerer Fläche, Klick in der Übersicht):
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
  1. ✅ **Einstellungen** (eingebaut): Tastatur (xkb-Belegung, Wiederholrate), Canvas-Größe, Raster-Abstand, Snap-Schwellwert, Lücken (`desk.layout { gaps = 16 }`), Haltezeit der Lös-Geste, Farben (Hintergrund, Raster, Akzent). Gelesen beim Start und **neu geladen, sobald die Datei gespeichert wird** (einmal pro Sekunde wird nachgesehen); Tippfehler, falsche Typen und Werte außerhalb des Bereichs sind Fehler (mit Einstellung, Datei und Zeile im Log), dann gelten beim Start die Standardwerte, im laufenden Betrieb bleiben die bisherigen. Wird die Fläche kleiner, rücken Cluster außerhalb als Ganzes herein. Lua 5.4 ist in die Binary eingebaut (`mlua`, vendored). `INFINITYDESK_CONFIG` zeigt auf eine andere Datei (zum Testen). Touchpad (`desk.input { touchpad = { tap, natural_scroll, dwt, accel_speed, accel_profile } }`, nur eigenständig; Standard wie bei Pyro in niri: Tippen, natürliches Scrollen, aus beim Tippen, Beschleunigung 0). Maus (`desk.input { mouse = { natural_scroll, left_handed, accel_speed, accel_profile } }`, nur eigenständig, 21.11.2026; Standard wie libinput, Pyros Werte aus niri (flach, 0.45) gehören in Pyros eigene Config). Noch nicht: Zoom-Verhalten.
  2. ✅ **Tastenbelegung** (eingebaut): `desk.bind("Mod+Return", desk.spawn("alacritty"))`, `desk.unbind("Mod+Tab")`. Die eingebauten Kürzel (siehe Punkt 6) bleiben, eigene gehen vor oder kommen dazu. Modifikatoren Mod/Super/Alt/Ctrl/Shift, Tasten als xkb-Namen; "Shift+Q" meint die Q-Taste. Aktionen: `desk.spawn("Befehlszeile")` (über `sh -c`, also mit Argumenten), `desk.close_window`, `desk.fullscreen`, `desk.layout_menu`, `desk.focus/swap/resize/pan("left"|"right"|"up"|"down")`, `desk.zoom_in`, `desk.zoom_out`, `desk.center`, `desk.overview`, `desk.recent_windows`. Lua-Funktionen als Aktion erst mit Stufe 4.
     ✅ **Maus-Bedienung:** belegbar über `desk.bind` (Punkt 6).
  3. ✅ **Fensterregeln** (22.11.2026, Pyros Auswahl): `desk.window_rule { match = { app_id = "…", title = "…" }, width = "half"/"full", height = "half"/"full", open_fullscreen = true, dock = false }`. Erkennung über reguläre Ausdrücke wie in niri (Suche, `^…$` für genau); alle passenden Regeln zählen, spätere gewinnen. `dock = false`: frei in der Bildschirmmitte in eigener Größe (wie ein Dialog), nimmt keinen freien Vorlagen-Platz. `open_fullscreen`: zuerst normal platziert, Mod+F führt dorthin zurück. Größen gelten nicht für Dialoge. Nicht gewählt (Pyro): fester Platz auf der Fläche je Programm; "in Cluster Z" auch nicht.
  4. **Ereignisse & Skripting:** Lua-Funktionen, die auf Ereignisse reagieren (Fenster geöffnet, Cluster gebildet …).
     - Pyro (22.11.2026): alle Ereignisse („mit Lua theoretisch auf alles reagieren, so viel Freiheit wie möglich“) und alle Fähigkeiten: Funktionen auf Tasten, Aktionen ausführen, Fenster abfragen und steuern, Befehle von außen. Vorbilder angesehen: Hyprland (Lua-Config mit `hl.on`, `hl.dispatch`, Wächter mit Zeitlimit, `hyprctl eval`) und niri (`niri msg`, JSON über `$NIRI_SOCKET`, Ereignis-Strom mit vollem Zustand vorab).
     - Ausnahme: keine Ereignisse pro Bild oder pro Mausbewegung (Lua-Aufrufe hunderte Male pro Sekunde würden das Zeichnen bremsen).
     - ✅ Schritt 1 (22.11.2026): Lua-Zustand bleibt geladen (beim Neuladen ersetzt). `desk.on("window.open", f)` gibt eine Nummer zurück, `desk.off(nummer)`. Ereignisse: window.open/close/focus/title/fullscreen, cluster.dock/undock, view.overview, config.reloaded, desk.start; Fenster als Tabelle `{ id, app_id, title }`. Funktionen auf Tasten (`desk.bind("Mod+G", function() … end)`), `desk.run(aktion)` führt sofort aus (wie `hl.dispatch`). Ereignisse werden gesammelt und zwischen zwei Runden der Ereignisschleife verteilt, nur wenn jemand zuhört; mehr als 32 Runden sich gegenseitig auslösender Ereignisse werden abgebrochen. Wächter: 100 ms pro Aufruf, 1,5 s fürs Laden. `print` schreibt ins Log. Funktionen nur auf Tasten, nicht auf Maustasten.
     - ✅ Schritt 2 (22.11.2026): Fenster als Tabelle `{ id, app_id, title, x, y, width, height, focused, fullscreen }` (auch in den Ereignissen). `desk.windows()`, `desk.focused()`, `desk.focus_window(id)` (Ansicht rückt mit), `desk.close(id)`, `desk.move(id, x, y)` (der ganze Cluster gleitet, bleibt im Canvas; beendet Vollbild nur, wenn es den Cluster betrifft). Rückgabe false, wenn das Fenster weg ist. Wie `desk.run` nur in Funktionen von `desk.on`/`desk.bind`, nicht beim Laden der Config (dann gibt es noch keine Fenster). Verschieben kann Fenster überlappen lassen, wie Ziehen mit der Maus. Timer erst, wenn jemand sie braucht.
     - Schritt 3: Befehle von außen.
       - ✅ 3a (22.11.2026): `infinitydesk msg eval '<lua>'` (wie `hyprctl eval`). Unix-Socket `$XDG_RUNTIME_DIR/infinitydesk.<wayland-N>.sock`, Pfad für gestartete Programme in `$INFINITYDESK_SOCKET` (wie `$NIRI_SOCKET`). Sicherheit: nur in `$XDG_RUNTIME_DIR` (Ordner nur für den Benutzer; fehlt die Variable, gibt es keinen Socket statt eines Ausweichens nach /tmp), Socket mit Rechten 0600. Keine zusätzliche Prüfung des Gegenübers (SO_PEERCRED bräuchte in Rust eine Zusatzbibliothek; niri verlässt sich ebenso auf den Ordner). Der Code läuft im geladenen Lua-Zustand wie eine Tastenfunktion (gleiche Fähigkeiten, gleiches Zeitlimit, `desk` ist schon da), die Rückgabewerte kommen als JSON zurück, eine Zeile pro Wert; Fehler auf stderr mit Exit-Code 1. Die Anfrage wird ohne Blockieren gelesen. Eigene kleine JSON-Ausgabe statt serde (Tabellen 1..n werden Listen, sonst Objekte).
       - ✅ 3b (22.11.2026): `infinitydesk msg events` (wie niris Event-Stream). Eine JSON-Zeile pro Ereignis, dieselben Ereignisse und Fenster-Felder wie `desk.on`: `{"event":…,"windows":[…],"on":…}`. Zuerst der Zustand: eine `state`-Zeile mit allen Fenstern, dann `view.overview` (offen oder nicht), damit Leisten mit demselben Code beides lesen. Ereignisse werden auch ohne Lua-Zuhörer gesammelt, sobald ein Programm zuschaut. Ein Programm, das nicht mehr liest (Puffer voll oder beendet), wird fallen gelassen, statt InfinityDesk aufzuhalten.
     - ✅ (22.11.2026) Testbetrieb gebaut: `infinitydesk --headless` (gedachter Bildschirm 1920×1080, nichts wird gezeichnet, Programme bekommen 60-mal pro Sekunde ihr Bild-Signal) und `tests/headless.rs`: jeder Test startet InfinityDesk in einem eigenen Ordner (eigenes `XDG_RUNTIME_DIR`), öffnet Test-Fenster (ein kleines Wayland-Programm im Test, zeichnet in der bestellten Größe, schließt auf Wunsch) und prüft über `infinitydesk msg`. Läuft mit `cargo test`, also auch in der GitHub-CI. Nicht abgedeckt: Maus und Tastatur (es gibt keine Eingabegeräte; Aktionen gehen über `desk.run`), Zeichnen und Gefühl. Die erste Runde: Fenster in Hälften, Ereignis-Reihenfolge, Vollbild hin und zurück, `desk.move`, `desk.close`, Endlosschleife, Ereignis-Strom.
     - Ursprünglich (Pyro, 22.11.2026): **Testbetrieb ohne Bildschirm** wie bei niri (Headless-Backend, Test-Clients) und Hyprland (`hyprtester` mit HEADLESS-Monitoren, gesteuert über `hyprctl`). Die Tests steuern InfinityDesk über `infinitydesk msg`, so kann Claude Ende-zu-Ende testen, ohne Pyros Desktop zu brauchen. Pyro testet dann nur noch Gefühl (Maus, Animationen) und Hardware.
- ✅ **GitHub-CI** (Pyro, 22.11.2026): Jeder Push wird geprüft (Formatierung, Clippy-Warnungen als Fehler, Tests), wie bei niri und Hyprland.
- ✅ **Fast alles Wichtige einstellbar, wie in Hyprland** (Pyro, 21.11.2026). Dieses Repo ist die **Basis** mit allgemeinen Standardwerten; Pyros eigene Einstellungen kommen später in eine eigene Lua-Config in einem eigenen Dotfiles-Repo. Geplant (ungefähr in dieser Reihenfolge): ✅ Maus, ✅ Autostart (`desk.spawn_at_startup("…")`, wie niri; nur beim Start, nicht beim Neuladen), ✅ Umgebungsvariablen für gestartete Programme (`desk.env { NAME = "wert" }`, wie Hyprland `env`; `WAYLAND_DISPLAY` setzt immer InfinityDesk), ✅ Zeiger-Thema und -Größe, ✅ XWayland, ✅ Maus-Belegung, ✅ Monitore (22.11.2026: `desk.output { name, mode = "BxH@Hz", scale, transform }`, ohne Namen für jeden Monitor; Skalierung auch gebrochen wie 1,5, Programme bekommen sie über wp-fractional-scale; alles wirkt sofort, auch Auflösung und Drehung. Genestet zählt nur die Skalierung. Position kommt mit mehreren Monitoren), Fensterregeln (Stufe 3). Dazu kleine Verhaltens-Werte, wo sie fehlen: ✅ Zoom-Geschwindigkeit (`desk.canvas { zoom_speed = 1.0 }`), ✅ Mausrad-Schwenk (`wheel_pan = 60`), ✅ Einrasten der Ansicht an/aus (`desk.snapping { view = true }`), 22.11.2026.
- ✅ **Ein universeller Baukasten** (Pyro, 22.11.2026): InfinityDesk soll eine Grundlage sein, auf der jeder sein eigenes Setup aufbaut. Also allgemeine, einstellbare Mechanismen mit vernünftigen Standardwerten statt Zuschnitt auf einen Rechner. Reihenfolge danach (Pyros Auswahl): Mod-Taste einstellbar, Monitor-Einstellungen, Animationen, Ereignisse & Skripting.
  - ✅ Animationen, Schritt 1 (22.11.2026): Die Ansicht gleitet, wenn ein Befehl sie versetzt (Fokus, Tauschen, Größe per Taste, Zentrieren, Übersicht, Alt+Tab, Vorlagen, Vollbild, neues Fenster, Einrasten nach dem Schwenken, Schwenken/Zoomen per Taste). 250 ms, zum Ende langsamer (kubisch), `desk.animation { duration_ms }`, 0 = springt. Beim Zoomen wächst die gezeigte Fläche gleichmäßig, damit nichts seitlich ausschwenkt. Der nächste Befehl rechnet vom Ziel aus und gleitet von dort, wo die Ansicht gerade ist. Von Hand (Ziehen, Rad, Touchpad) folgt die Ansicht sofort und hält ein Gleiten an. 
  - ✅ Animationen, Schritt 2 (22.11.2026): Fenster gleiten, wenn ein Befehl sie versetzt (Tauschen, Nachbarn rücken beim Größenwechsel, Vorlagen, andere Cluster weichen aus, Andocken beim schnellen Loslassen), gleiche Dauer und Kurve. Nur das Bild gleitet (Versatz beim Zeichnen), Andocken, Klicks und Fokus nutzen sofort den neuen Platz. Der Ausgleich, mit dem eine festgehaltene rechte/untere Kante bei neuer Größe stehen bleibt, ist sofort; Ziehen von Hand ebenso (Greifen beendet ein Gleiten). Größenwechsel selbst werden nicht animiert (das Programm zeichnet die neue Größe).
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

## 6d. Optik (Pyro, 22.11.2026)

Pyros Auswahl: keine Titelleisten, Rahmen ums aktive Fenster, abgerundete Ecken (Radius in der Config, 0 = eckig). Schatten nicht.
- ✅ **Keine Titelleisten** (xdg-decoration): Programme, die es können (kitty, alacritty, Qt), lassen ihre Titelleiste weg; InfinityDesk zeichnet auch keine, so wirken Cluster wie aus einem Guss. Schließen mit Mod+Q. Zusätzlich gelten alle Fenster als „gekachelt“, dann lassen GTK-Programme Schatten und runde Ecken weg. `desk.layout { titlebars = true }` lässt jedes Programm selbst entscheiden. Gilt für Fenster, die nach dem Ändern aufgehen. GTK-Programme behalten ihre Kopfleiste (sie ist bei GTK Teil des Programms).
- ✅ **Rahmen ums aktive Fenster** (`desk.layout { border = 3 }`, Farbe `desk.colors { border = "#b8bcc8" }`, 0 = kein Rahmen): liegt außen ums Fenster (in der Lücke), gleich breit bei jedem Zoom, gleitet mit dem Fenster. Er liegt direkt hinter seinem Fenster, also unter Fenstern davor. Hellgrau statt Akzentblau, damit markierte Fenster (blau) unterscheidbar bleiben. Im Vollbild kein Rahmen.
- ✅ **Abgerundete Ecken** (`desk.layout { corner_radius = 0 }`, Standard eckig): Radius in Canvas-Pixeln, wird also beim Herauszoomen mitverkleinert. Die Fenster werden auf der Grafikkarte rund abgeschnitten (eigener Shader, rechnet über die Projektion des Bildes, stimmt daher auch auf gedrehten und gespiegelten Bildschirmen; genestet geprüft mit gespiegelt, normal und 90°). Dabei fällt auch alles außerhalb des Fensters weg (Schatten der Programme), Menüs nicht. Rahmen und Markierungen folgen den runden Ecken. Vollbild bleibt eckig (und kann dann direkt auf den Bildschirm). Nach dem Neuladen der Config wird einmal alles neu gezeichnet, damit geänderte Ecken sofort stimmen.

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
- ✅ Maus-Einstellungen in der Config (21.11.2026, siehe 6c).
- Pyro will (21.11.2026) alle vier: ✅ Maus-Einstellungen, ✅ Autostart, ✅ XWayland, mehrere Monitore (Bedienung entschieden, siehe Punkt 8; gebaut, sobald ein zweiter Bildschirm zum Testen da ist). Die Bildrate stimmt schon: eigenständig nimmt InfinityDesk die eigene Auflösung des Monitors mit der höchsten Bildrate dort.
- ✅ XWayland (21.11.2026) über xwayland-satellite wie niri: InfinityDesk belegt selbst ein freies X-Display (Lock-Datei, Socket-Datei und abstrakter Socket; ist der abstrakte Socket fremd belegt, gilt das Display als besetzt, sonst könnte ein anderer Benutzer mithören; verwaiste Locks toter Prozesse werden wiederverwendet; `/tmp/.X11-unix` muss root gehören und das Sticky-Bit haben) und reicht den Socket per `-listenfd` weiter. So können X11-Programme aus dem Autostart sofort verbinden. Gestartete Programme bekommen `DISPLAY`; ohne xwayland-satellite wird `DISPLAY` entfernt (nie das X-Display des Hosts). X11-Fenster bekommen dieselbe halbe Breite wie Wayland-Fenster (geprüft mit glxgears; ein früherer Verdacht kam vom Test: das Programm startete, bevor das genestete Fenster Vollbild war). Offen: stürzt xwayland-satellite ab, wird es nicht neu gestartet.
- ✅ Monitor-Einstellungen in der Config (22.11.2026, `desk.output`, siehe 6c).
- Noch nicht: mehrere Monitore, Monitor ab-/anstecken im Betrieb.

## 8. Offene Fragen

- Bedienung des Ganzen (Punkt 6)
- ✅ Mehrere Monitore (Pyro, 22.11.2026): **ein gemeinsamer Canvas, jeder Monitor hat seine eigene Ansicht**, schwenkt und zoomt für sich; Fenster zieht man von einem Monitor auf den anderen. Verworfen: alle Monitore als ein großer Bildschirm (schwenken gemeinsam), je Monitor ein eigener Canvas. Gebaut wird es, wenn ein zweiter Bildschirm zum Testen da ist (bei Pyro ist nur einer angeschlossen).
- ✅ Neue Fenster (eingebaut): zuerst füllen sie freie Plätze der letzten Vorlage (Mod+R). Sonst docken sie am **aktiven Fenster** an: rechts daneben (Oberkanten bündig), sonst darunter (linke Kanten bündig), links, oben. Seiten, die auf dem Bildschirm liegen, kommen zuerst; ist nirgends auf dem Bildschirm Platz, dockt es trotzdem an der ersten freien Seite an und die **Ansicht rückt gerade so weit mit**, dass es ganz zu sehen ist. Ist kein Fenster aktiv (vorher auf leere Fläche geklickt), kommt es in die linke Bildschirmhälfte (seit 21.11.2026, vorher frei in der Mitte). Neue Fenster sind halb so breit und so hoch wie der Bildschirm (mit Lücken), zwei neue Fenster füllen ihn also genau. Dialoge erscheinen immer frei in der Mitte und wählen ihre Größe selbst. Ein Launcher (fuzzel) ändert das aktive Fenster nicht. 💬 Feinabstimmung beim Ausprobieren.
