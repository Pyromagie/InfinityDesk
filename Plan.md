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

- 💬 Einrasten an Kanten und Ecken innerhalb eines Schwellwerts (Vorschlag ~15 px).
- ✅ Resize einer geteilten Kante, zwei Modi:
  - **Standard:** Nachbarn im Cluster wachsen/schrumpfen mit (der Viewport ist nur ein Bildschirm, daher nicht einfach wegschieben).
  - **Mit Shift (o. ä.) gehalten:** nur das gezogene Fenster ändert seine Größe, das Nachbarfenster (und alles dahinter) wird verschoben.
  - ✅ "Verschoben" gilt für alles, was auf der anderen Seite der Kante im Cluster hängt. Das Cluster bleibt zusammen, wird nur größer/kleiner.
- ✅ Fenster löst man per **Ruck** (schnelles Wegziehen) aus dem Cluster. 💬 Schwellwert/Gefühl beim Testen feinjustieren.
- ✅ Orangener Punkt in der Skizze: ohne Bedeutung (Artefakt vom Zeichenprogramm).

## 6. Navigation

- ✅ Pan **und** Zoom, damit man sich schnell einen Überblick über die ganze Fläche verschaffen kann.
- ✅ **Zoom-Grenze rein:** maximal 1:1 (Zoomfaktor 1.0, Fenster in ihrer normalen Pixelgröße). Man kann also nie in ein Fenster "hineinzoomen". Das passt sich an die Display-Auflösung an (bei 1920×1080 ist 1:1 die Grenze), damit man nicht auf wenige Pixel eines Fensters zoomen kann.
- ✅ **Zoom-Grenze raus:** so weit, dass die ganze Canvas-Fläche auf den Bildschirm passt.
- 💬 Bedienung (Maus / Tastatur / Touchpad-Gesten) – später.
- Vorläufige Test-Bedienung (Platzhalter, bis die Bedienung entschieden ist):
  - Linksziehen auf leerer Fläche: Fläche verschieben (Pan)
  - Alt + Linksziehen auf einem Fenster: Fenster verschieben
  - (Super ist im genesteten Betrieb von niri belegt, daher Alt)

## 7. Roadmap

1. ✅ Minimaler Compositor (genestet, winit), zeigt Wayland-Clients an
2. ✅ Fenster auf dem Canvas platzieren, Viewport verschieben
3. Snapping + Cluster
4. Cluster-Resize, Culling, Zoom, Cluster-Fokus
5. Bedienung, Konfiguration, Feinschliff

## 8. Offene Fragen

- Bedienung des Ganzen (Punkt 6)
- Mehrere Monitore: ein Canvas pro Monitor oder ein gemeinsamer?
- ✅ Neue Fenster: ist **kein Cluster fokussiert**, erscheint das Fenster in der Mitte des Bildschirms. Ist ein **Cluster fokussiert**, dockt es neben den Fenstern an, die schon im Cluster sind. Stelle: **rechts neben dem zuletzt fokussierten Fenster**; ist dort kein Platz, die nächste freie Seite. Muss im Viewport sichtbar bleiben. 💬 Feinabstimmung beim Ausprobieren.
