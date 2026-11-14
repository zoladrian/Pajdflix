# Informacja o licencjach

Pajdflix jest zmienioną wersją projektu Stremio Native. Przy rozdawaniu programu zostają obowiązki z licencji poniżej. To opis plików z tego repozytorium, nie porada prawna.

## Program

Katalog `stremio-native` jest na **GPL-3.0-only**. Pełny tekst jest w [`stremio-native/LICENSE`](stremio-native/LICENSE) i jest kopiowany do instalacji jako `licenses/pajdflix`.

Oryginał: [stremio-native/stremio-native](https://github.com/stremio-native/stremio-native), autor wydania upstream perpetus i współtwórcy. Pajdflix dopisuje własne zmiany na tej samej licencji i oznacza się jako wersja zmieniona. Prawa autorskie do oryginału zostają przy jego autorach.

Ta sama licencja obejmuje kod Pajdflix dołączony do tego programu. Nie da się opublikować samej binarki na innej licencji.

## Serwer strumieni

Katalog `stream-server` jest na **MIT**, copyright (c) 2025 perpetus. Tekst: [`stream-server/LICENSE`](stream-server/LICENSE). Informacja o prawach autorskich i ten tekst zostają przy kopiach. W połączonym programie Pajdflix warunki GPL-3 obejmują całość binarki.

## Inne części

- **libmpv**, dołączany do instalacji Windows, jest na GPL albo LGPL. Instalator kładzie `licenses/mpv/LICENSE.GPL` i `LICENSE.LGPL` obok programu.
- **Slint** w `stremio-native/vendor` jest dostępny na GPL-3.0 albo na osobnych licencjach SixtyFPS. Ta aplikacja idzie ścieżką GPL-3, więc plakietka licencji royalty-free nie jest wymagana. Nagłówków licencji Slint nie usuwamy.
- **ThumbFast** (podgląd klatek) jest na MPL-2.0. Notka: [`stremio-native/licenses/thumbfast`](stremio-native/licenses/thumbfast).
- **UnRAR** w `stream-server` zostaje przy swoim tekście licencji. Nie służy do odtwarzania formatu RAR jako archiwizatora.

## Znak towarowy

Stremio jest znakiem SmartCode Ltd. Pajdflix nie jest z nimi powiązany. Zgodność z protokołem dodatków nie oznacza zgody ani sponsorowania.
