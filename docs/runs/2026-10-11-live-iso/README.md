# Первый установочный ISO, 2026-10-11

Собран `looom-0.2.0-alpha.1-2026.10.11-x86_64.iso` с готовым native Rust
менеджером, терминальным welcome-меню и онлайн-установщиком. Сборка и все
продуктовые проверки выполнялись **только в VM**. Локально редактировались
исходники, управлялись VM, передавались публичные файлы и сравнивались SHA.
Существующие desktop VM и Intel N100 не изменялись.

ISO: **1 706 952 704 байта** (около 1,59 GiB), SHA-256:

```text
b090aff952a2520adbf176e2391ad8e262c7c4f3ae304393c7d7e90a296eb4bd
```

Rust-бинарник из проверенного `b192d28` не изменялся:

```text
d76cdb361bde3f7d9469734fb8f23e98b0b812fd68cb72b5cf2ddd9ce361e9db
```

[Краткая инструкция](../../live-iso.md),
[скрипт сборки](../../../scripts/build-iso.sh),
[VM-контроллер](../../../scripts/test-live-iso.py).

## Сборка

Development VM `Cachyos`, MAC `52:54:00:7b:23:63`, 6 CPU/8 GiB, была выключена
и запущена для этого этапа. Исходно свободно 35 GiB. Создан отдельный writable
снимок `@iso-builder-20261011` из `@toolchain-20261006`; рабочий read-only релиз
и исходный toolchain не обновлялись. Builder смонтирован в
`/var/lib/looom/dev/iso-20261011/builder`, публичные входы — в соседнем `source`
и bind-mounted как `/workspace` внутри chroot.

Только builder обновлён из подписанного Arch Linux Archive `2026/10/07` с
установкой `archiso 91-1`. Профиль основан на releng именно этого пакета;
live-среда содержит 455 пакетов. Overlay включает welcome-меню, справку,
шаблоны base/apps и точный проверенный бинарник. Установленная система использует
Limine/UKI; для запуска самого live-носителя оставлен стандартный UEFI backend
Archiso. BIOS не включён.

```sh
arch-chroot /var/lib/looom/dev/iso-20261011/builder \
  env LOOOM_SOURCE_REVISION=b192d28 \
  bash /workspace/scripts/build-iso.sh \
  /root/looom-iso /iso-work-final /iso-out-final
```

[Полный финальный журнал](build/build-final.log),
[манифест и SHA файлов профиля](build/looom-0.2.0-alpha.1-2026.10.11-build.txt),
[точный список пакетов](build/looom-0.2.0-alpha.1-2026.10.11-packages.txt),
[контрольная сумма ISO](build/looom-0.2.0-alpha.1-2026.10.11-x86_64.iso.sha256).
Манифест фиксирует входы; побитовая воспроизводимость пока не заявляется.

Первая сборка действительно загрузилась в UEFI и показала меню, но запуск
looom отказал с `PermissionError`: Archiso нормализовал mode бинарника до 0644.
Исправление — явное `file_permissions["/usr/bin/looom"]="0:0:755"` в профиле.
Образ пересобран в новом work-каталоге, все проверки ниже выполнены на финальном
SHA `b090…`. Первая сборка `c972…` не используется и не публикуется.

## Настоящая загрузка ISO и установка

Создана отдельная `Looom-iso-20261011`, MAC `52:54:00:11:10:11`, 4 CPU/8 GiB,
UEFI/OVMF, новый пустой VirtIO-диск 64 GiB без serial/WWN и отдельные NVRAM.
Её адрес закреплён за MAC: `192.168.122.155`. Старые диски и VM не использовались.

Финальный ISO подключён как CD-ROM и действительно загружен своим ядром
`7.2.9-arch1-1`. В serial-консоли после загрузки добавлен публичный management
ключ только в RAM и включён SSH для приёмки. Эти настройки в ISO отсутствуют.
SquashFS payload проверен отдельно: machine-id имеет состояние `uninitialized`,
готовых SSH host keys и management authorized_keys нет.

```sh
python /run/looom-iso-test/scripts/test-live-iso.py inspect
python /run/looom-iso-test/scripts/test-live-iso.py install
```

Контроллер проверяет root, виртуализацию, конкретный MAC, UEFI, размер vda,
SHA и исполнимость менеджера. PTY-проверка покрывает запуск мастера из меню,
выбор диска по номеру и отмену с возвратом в меню без изменения начала диска.
Loop/CD-ROM не показываются как установочные диски.

Полный мастер выполнен с PTY, номером диска `1`, hostname `looom-iso-test`,
пользователем `kexfff`, `YES` и скрытым вводом паролей. Private fixture создан
внутри этой live-VM, в закрытом `/run/looom-iso-test/private`; пароли не вошли
в ISO, репозиторий или публичные логи. После выключения live-среды plaintext
fixture исчез вместе с RAM.

Скачивание началось с пустым кэшем. Для ускорения на исходной development VM
проверены хеши архивов/подписей её старого lock из **того же** `2026/10/07`;
переданы только публичные пакетные файлы. 424 отсутствующих файла добавлены
атомарными hardlink из staging, текущие загрузки не заменялись. Перед стиранием
диска установщик заново проверил подписи и полный набор **679 пакетов**.
Поэтому этот прогон не заявляет полную установку исключительно с холодным
интернет-кэшем. Проверяются реальные ISO/kernel/userspace и весь движок установки.

Получены GPT, ESP 2 GiB, Btrfs с `@bootstrap`, `@home`, `@var`, `@state`,
read-only `@root-initial`, Limine и отдельная прямая UEFI recovery-запись.
Движок завершил все 12 этапов, оставил first-boot one-shot на `initial` и
не перезагружал систему автоматически.

[Проверки live-среды](live/inspect.log), [payload](live/payload.log),
[кэш](live/cache-prefetch.log), [полный мастер](live/install.log),
[итог контроллера](live/install-controller.log), [SHA контроллеров](live/controllers.sha256),
[публичный аудит](live/public-audit.log), [меню ISO](live/menu.png).

## Первая загрузка и рабочий стол

VM выключена штатно, ISO извлечён, система загружена с диска. Реальный
LoaderInfo — **Limine 12.9.3**, root — `/@root-initial`, read-only.
`looom verify` проверил ядро, инвентарь пакетов, mounts, units и credentials;
`looom accounts check` прошёл. `base.yaml`, `base.lock` и `apps.yaml` находятся
в `/home/kexfff/looom`, принадлежат UID/GID 1000. Flatpak, Distrobox, Podman
и FUSE включены; пользовательская `looom apps check` проходит.

В пакетном журнале fontconfig вызвал `vercmp` до установки pacman, получив
`command not found` и предупреждение арифметики; затем его cache rebuild
продолжился. Итог проверен отдельно: `/etc`, `/var`, `/var/cache` и fontconfig
cache имеют mode 0755, семь файлов кэша существуют, `fc-match` от UID 1000
выбирает Noto Sans. Это предупреждение процесса первой установки, не ошибка
завершённой приёмки: [проверка шрифтов и прав](boot/fonts-and-permissions.log).

Выполнен настоящий вход через SDDM: сессия `Type=wayland`, plasmashell UID 1000,
PipeWire и WirePlumber активны. Затем `looom confirm initial` подтвердил релиз.
После обычного reboot получен **другой boot_id**, снова тот же read-only
`initial`, `saved_entry=looom-initial`, повторный verifier прошёл.

[Первая загрузка](boot/boot.log), [KDE и confirm](boot/confirm-and-desktop.log),
[повторная постоянная загрузка](boot/persistent-boot.log),
[рабочий стол](boot/plasma-desktop.png), [аудит на исходной VM](boot/boot-public-audit.log).
Журналы перед экспортом проверены против credentials именно той VM,
которая их создала. Credentials и закрытые ключи между VM не переносились.

VM оставлена запущенной в подтверждённом `initial` для пользовательского теста.
Код менеджера не изменён; прежние Rust-тесты и расширенные проверки приложений
не повторялись. На этом ISO не проверены реальная USB-загрузка/N100, Wi-Fi
или полная установка iNiR; пользовательский следующий этап — ежедневное
использование и обратная связь. Шифрование, Secure Boot, dual boot, CachyOS,
офлайн-установка и Calamares остаются вне этого выпуска.
