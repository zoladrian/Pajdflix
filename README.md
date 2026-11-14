# Pajdflix

**Film z torrenta, po polsku, bez kombinowania.**

Pajdflix jest od tego, żeby włączyć film albo serial i już go oglądać. Nie szukasz pliku, nie porównujesz dziesięciu wydań i nie składasz odtwarzacza. Wybierasz tytuł. Program sam znajduje torrent, bierze 1080p, woli polski dubbing, lektor albo napisy i puszcza seans, gdy tylko da się go wystartować. Seedowanie zostaje wyłączone.

## Jak to działa

1. Otwierasz katalog albo wpisujesz tytuł.
2. Klikasz **Oglądaj**.
3. Pajdflix odrzuca zwiastuny i pliki, które nie wyglądają na film, i zostawia polską wersję.
4. Ekran robi się pełny. Film idzie od razu, reszta dociąga się w tle.
5. Napisy z polskich dodatków są prostowane, więc wchodzą polskie znaki, a nie pytajniki.
6. Po seansie cache można sprzątnąć. Zostaje obejrzany film, nie sterta plików.

Ty tylko oglądasz.

To zmieniona wersja społecznościowego [Stremio Native](https://github.com/stremio-native/stremio-native). Nie jest aplikacją SmartCode i nie jest oficjalnym Stremio.

## Co jest w środku

- **Katalog i wyszukiwanie.** Strona główna, karta tytułu, biblioteka, profile i dodatki mówiące protokołem Stremio.
- **Odtwarzacz.** Obraz idzie przez mpv, z napisami, ścieżkami audio, skalowaniem kadru i trybem pełnoekranowym od startu.
- **Źródła.** Przycisk „Oglądaj” wybiera jakość (domyślnie sensowne 1080p) i woli polski dubbing, lektor albo napisy. Podejrzane pliki i zwiastuny nie startują same.
- **Torrenty w tym samym programie.** Pobieranie i odtwarzanie obsługuje wbudowany serwer. Seedowanie jest wyłączone.
- **Polskie napisy.** Pliki w Windows-1250, ISO-8859-2 i UTF-16 są zamieniane na UTF-8. Wersja, w której polskie litery już zniknęły, jest pomijana.
- **Porządek na dysku.** Obejrzany cache torrenta da się sprzątnąć po seansie. Własne pliki z dysku nie są przy tym ruszane.
- **Obraz.** Na karcie graficznej można włączyć filtry Anime4K albo lekki FSR. Domyślnie są wyłączone.

## Czym to się różni od Stremio

Oficjalne Stremio na komputer to interfejs webowy w oknie aplikacji i osobny serwer. Pajdflix jest jednym programem napisanym w Rust, z własnym interfejsem.

| | Stremio | Pajdflix |
|---|---|---|
| Okno | strona w osłonie desktopowej | natywny interfejs |
| Serwer strumieni | osobny proces Node.js | wbudowany, w tym samym programie |
| Język | wiele, interfejs webowy | polski interfejs od pierwszej sekundy |
| Start | okno w rozmiarze pulpitu | pełny ekran |
| Konto i synchronizacja Stremio | tak | nie polegamy na chmurze Stremio; biblioteka jest lokalna |
| Seedowanie | zależy od ustawień | wyłączone |
| Napisy polskie | jak je odda dodatek | dodatkowo prostowane kodowanie |
| Oficjalna aplikacja SmartCode | tak | nie |

Dodatki, których używasz w Stremio, da się podłączyć tutaj, jeśli mówią tym samym protokołem. Katalog Cinemeta, Torrentio czy dodatki z napisami są osobnymi usługami. Pajdflix ich nie zawiera i nie utrzymuje.

Części oficjalnego Stremio tu nie ma: synchronizacji konta między telefonem i komputerem, aplikacji mobilnej, telewizora ani sklepu dodatków w tej samej postaci. To odtwarzacz na Windows.

## Instalacja

Na Windows 10 lub 11, 64-bit. Procesor musi obsługiwać zestaw x86-64-v3 (AVX2). To praktycznie każdy komputer z ostatnich lat. Do interfejsu potrzebny jest sterownik z OpenGL.

1. Otwórz [wydania Pajdflix](https://github.com/zoladrian/Pajdflix/releases).
2. Przy najnowszej wersji pobierz instalator `PajdflixSetup-v…-x64.exe`. Pierwsze wydanie z tych źródeł to **1.0.0**, plik `PajdflixSetup-v1.0.0-x64.exe`.
3. Uruchom go. Instalacja jest dla bieżącego użytkownika i nie prosi o uprawnienia administratora.
4. Zaznacz skrót na pulpicie, jeśli chcesz mieć ikonę pod ręką. Ta opcja domyślnie nie jest włączona.
5. Na końcu kreatora możesz od razu uruchomić Pajdflix.

Program ląduje w `%LOCALAPPDATA%\Programs\stremio-native`. Biblioteka, cache i logi zostają w `%LOCALAPPDATA%\stremio-native`, także po zdjęciu programu. Dzięki tej ścieżce starszy profil nie ginie przy kolejnej instalacji.

Aktualizacja to ponowne uruchomienie nowszego instalatora z tej samej strony wydań. Pajdflix nie pobiera wydań obcego projektu jako własnych aktualizacji.

Deinstalacja jest w Ustawieniach Windows, przy aplikacjach, pod nazwą Pajdflix.

## Licencja

Pajdflix jest wolnym oprogramowaniem na **GNU GPL w wersji 3**. Możesz go używać, zmieniać i rozdawać, także pod inną nazwą, pod warunkiem że odbiorca dostaje tę samą licencję i dostęp do źródeł dokładnie tej wersji. Źródła wydania są w tym repozytorium.

Nie wolno zamknąć tej całości ani ogłosić jej własnością jednej osoby. Kod Stremio Native należy do jego autorów. Zmiany Pajdflix są dokładane na tej samej licencji.

Szczegóły i licencje części składowych są w [NOTICE.md](NOTICE.md).

Nazwa i znak Stremio należą do SmartCode Ltd. Pajdflix nie jest przez nich prowadzony, sponsorowany ani zatwierdzony.
