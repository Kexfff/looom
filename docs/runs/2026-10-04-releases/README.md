# Read-only релизы, откаты и KDE в VM

Дата начала: 2026-10-04. Выполняется по запросу пользователя продолжить
read-only релизы и откаты, затем перейти к KDE, если проверки успешны.
Статус: выполнено. VM оставлена в подтверждённом desktop-релизе и активной
Wayland-сессии codex. Исходный пароль восстановлен, временные секреты удалены.

## Принятые решения этого эксперимента

- Корни собираются заново через pacstrap из Arch Archive 2026/10/03.
  Сборщик не монтирует общие `/home`, `/var`, `@state` или ESP внутрь chroot.
  Разрешено совместное использование только кэша пакетов; UID/GID заранее закреплены.
- `/etc/passwd` и `group` остаются обычными декларативными файлами.
  `shadow` и `gshadow` ссылаются в `/run/looom/accounts`.
  Ранняя обязательная служба собирает парольные файлы из шаблонов релиза и credentials в `@state`.
  Все пароли в шаблонах заблокированы. Системные аккаунты создаются при сборке,
  systemd-sysusers при загрузке маскируется. Карта UID/GID дополняется без переназначения.
- Смена пароля: `sudo looom-password codex`. Обычный `passwd` и смена пароля
  через графические настройки пока не поддерживаются. Пароль обновляется атомарно
  в защищённом состоянии и сразу в runtime shadow.
- Локальные исключения `/etc`: каталог `looom-local` и
  `NetworkManager/system-connections`, подключённые из `@state` через bind mounts.
  `resolv.conf` ссылается на файл NetworkManager в `/run`.
- Machine-id закреплён в состоянии и копируется в каждый релиз при сборке;
  host keys SSH и ключ аварийного root-доступа находятся только в состоянии.
- v1 использует linux, v2 — linux-lts и дополнительный пакет tree.
  Это проверяет смену версии ядра внутри одного согласованного архива,
  а не обновление даты всех репозиториев.
- Неподтверждённая загрузка не меняет постоянный выбор GRUB. Для намеренно
  неисправного кандидата используется `systemd.unit=emergency.target`.
- Публикация сохраняет read-only корень, затем UKI на ESP, затем меню.
  Старое меню сохраняется рядом; bootstrap остаётся отдельной аварийной записью.
  Атомарный rename на FAT не является доказательством устойчивости при потере питания.

## Ход работы

1. Проверена запущенная VM: bootstrap, UEFI, 4 исходных подтома, 55 GiB свободно,
   нет failed units. Графика VirtIO, SPICE, звук ICH9 уже доступны.
2. Добавлены воспроизводимые сценарии сборки, предоставления учётных записей,
   выбора/подтверждения релиза и проверки реально загруженной системы.
3. На VM инициализированы постоянные credentials, SSH identity,
   явные локальные каталоги и карта UID/GID. Секреты не выводятся и не копируются в Git.
4. Собран и пробно загружен v1. Выявлено отсутствие passwd/group до local-fs,
   нарушающее ранний NSS. Кандидат отвергнут; следующая загрузка вернула bootstrap.
5. В v1r1 исправлен NSS. Выявлено ошибочное копирование mode 0700 registry на `/etc`.
   D-Bus не прочитал machine-id; кандидат также отвергнут, bootstrap восстановлен.
6. Сборщик исправлен: passwd/group доступны с начала загрузки, `/etc` 0755,
   добавлена предзагрузочная проверка чтения machine-id от пользователя dbus.
   Для новых сборок корень монтируется отдельно; приватные вложенные Btrfs подтомы
   systemd удаляются рекурсивно после публикации read-only снимка.
7. v1r2 реально загрузился без failed units; SSH codex и sudo прошли, полный
   verifier после исправления проверки завершающего `/` DBPath прошёл.
   В root `.ssh` выявлена ссылка внутри уже созданного каталога; исправлено через `ln -sT`.
8. Смена password credential и PAM login проверены; правильный пароль принимается,
   неправильный отклоняется. Исходный пароль восстановлен.
9. v1r3 проверен полностью через root SSH: `/` и `/etc` read-only, остальные
   обязательные монтирования read-write, next_entry пуст, kernel 7.2.8-arch1-2.
   Подтверждён как постоянный выбор looom-v1r3.
10. Исправлена очистка GPG-agent сборки и наследование flock descriptor.
    Для v1r3 только приватный mount был освобождён после завершённой валидации,
    опубликованный read-only корень не менялся.
11. v2 собран: linux-lts 6.18.54-2 и tree 2.3.2-1 из того же архива, 211 пакетов.
12. Прерванная публикация v2 после записи UKI сохранила старое меню и saved_entry.
    recover не добавил validated кандидат; повтор публикации прошёл.
13. Реальная загрузка v2 прошла полный verifier, kernel 6.18.54-2-lts, tree установлен,
    декларация updated-v2. Изменены локальный marker, файл пользователя и пароль.
14. Выполнен настоящий откат v2 -> v1r3: kernel 7.2.8-arch1-2, tree отсутствует,
    декларация baseline-v1. Локальный marker и пользовательский файл сохраняются,
    UID/GID 1000:1000; PAM принимает новый пароль. Исходный пароль восстановлен.
15. broken опубликован и выбран только однократно при saved_entry=looom-v1r3.
    Нормальный SSH не появился. После host reset VM вернулась в v1r3;
    следующий полный verifier прошёл, next_entry пуст. broken отвергнут и убран из меню.
16. `LOOOM_FAIL_AFTER=build` прервал приватную сборку interrupted с кодом 90.
    Ни read-only корень, ни published metadata, ни запись меню не появились.
    recover сообщил о building операции и сохранил доступный v1r3.
17. Начата свежая desktop-сборка: 652 пакета, 488 MiB дополнительных архивов.
    Для неё root SSH подключается bind mount вместо ссылки, чтобы tmpfiles
    мог работать с каталогом. NetworkManager использует rc-manager=unmanaged,
    продолжая генерировать DNS в `/run` без попыток записи в read-only `/etc`.
18. Финализация desktop остановилась на ложном совпадении `volume_key` с фильтром
    `*_key`. Фильтр исправлен на секретные пути и добавлен отдельный finalizer.
    Из уже проверенной сборки создан read-only @root-desktop; пакетная установка
    не повторялась. Finalizer проверил 652 версии, locked shadow и неизменность UKI.
19. desktop реально загрузился, полный verifier прошёл, SDDM greeter работает.
    Plasma 6.7.5-1, SDDM 0.21.0-7, PipeWire 1:1.6.9-1, WirePlumber 0.5.18-1.
20. PAM SDDM проверен; временный пароль передан в поле greeter через libvirt
    без хранения на хосте и без plaintext в аргументах. SDDM авторизовал codex
    и запустил Wayland session. Plasmashell, KWin Wayland, PipeWire, Pulse и
    WirePlumber active; user failed units отсутствуют. desktop подтверждён.
21. Через kwriteconfig6 добавлен тестовый marker в kdeglobals; выполнен откат
    desktop -> v1r3. SDDM отсутствует в списке пакетов консольного релиза;
    KDE marker и изменённый пароль сохраняются, PAM login проходит.
22. Выполнен возврат v1r3 -> desktop. Read-only verifier и PAM SDDM прошли,
    выполнен повторный графический вход. Все 5 пользовательских служб active,
    Wayland session active на seat0, marker сохранён. Исходный пароль восстановлен,
    каталог private-password-test удалён. Desktop снова подтверждён как saved_entry.

## Итог проверок

| Проверка | Результат |
| --- | --- |
| Корень и декларативный `/etc` read-only, Btrfs ro property | PASS |
| `/home`, `/var`, `@state`, локальные bind mounts read-write | PASS |
| Пакетная база находится внутри выбранного релиза | PASS |
| v1r3 -> v2 -> v1r3: kernel, tree, декларация | PASS |
| Локальные файлы, UID/GID и пароль сохраняются при откате | PASS |
| PAM login и SDDM: верный пароль принимается, неверный отклоняется | PASS |
| Прерывание публикации после UKI, recover и повтор публикации | PASS |
| Прерывание private build не создаёт пункт меню | PASS |
| Неисправный неподтверждённый старт -> host reset -> v1r3 | PASS |
| Реальный SDDM вход, Plasma Wayland, KWin и Plasmashell | PASS |
| PipeWire, PipeWire Pulse и WirePlumber active | PASS |
| Desktop -> console -> desktop сохраняет KDE marker и пароль | PASS |

Звук проверен на уровне служб; физическое воспроизведение и микрофон не проверялись.
Корни v1, v1r1, v1r2, broken сохранены отвергнутыми для диагностики и исключены из меню.
Приватный @build-interrupted также сохранён, не является загрузочным релизом.

## Артефакты и воспроизводимость

- `evidence/verify-v1r3.log`, `verify-v2-and-select-rollback.log`,
  `rollback-v2-to-v1r3.log`: реальные Boot ID, корни, ядра и состояния GRUB.
- `evidence/publication-interruption.log`, `build-interruption.log`,
  `broken-selection.log`, `broken-fallback.log`: проверки прерываний и fallback.
- `evidence/desktop-*`, `verify-desktop-before-login.log`:
  проверки графического входа, user services и round trip.
- `evidence/sddm.png`, `desktop-final.png`, `broken-screen.png`: фактические экраны VM.
- `evidence/build-*.log`: полные журналы всех сборок, включая промежуточные ошибки.
- `evidence/ID/`: полный inventory, explicit packages, fstab, cmdline,
  хеши архивов/подписей и UKI, профиль и lock каждого построенного кандидата.
- `evidence/v2/builder-executed.sh`: фактическая версия сборщика v2 до последующих правок.
- `evidence/broken/source`, `interrupted/source`, `desktop/source`:
  сохранённые версии сценариев и конфигураций, актуальные при этих сборках.
- Desktop собран до исправления фильтра volume_key; отдельная финализация выполнена
  новой версией `scripts/finalize-release.sh`, её хеш в `finalization-source.sha256`.
- `manifests/releases/`: подтверждённые metadata и точные package locks
  v1r3 (210 пакетов), v2 (211), desktop (652).

В ранних версиях v1/v1r1/v1r2/v1r3 полный source snapshot автоматически не сохранялся;
профили, пакеты, UKI hashes и журналы сохранены. Текущие сценарии включают исправления
и предназначены для повторения финального поведения на новой bootstrap VM.
Цель воспроизводимости — точные пакеты и конфигурация; побитовая идентичность UKI
и образа не заявляется.

SSH private key находится только в игнорируемом `.local/vm-access`; credential hashes
только в защищённом состоянии VM. Пароли, shadow и временный тестовый пароль
не входят в сохранённые артефакты.

## Ограничения

Этот прототип не реализует очистку релизов, полный YAML API и автоматический watchdog.
Отсутствие подтверждения возвращает предыдущий выбор при следующем старте;
зависший компьютер сам по себе не перезапускается.
Общее `/var` не откатывается: совместимость данных сервисов требует отдельных проверок.

## Источники технических решений

- [systemd: зависимости служб ранней загрузки](https://github.com/systemd/systemd/blob/main/man/systemd.service.xml).
- [systemd: RequiresMountsFor и зависимости](https://github.com/systemd/systemd/blob/main/man/systemd.unit.xml).
- [ArchWiki: установка и запуск KDE Plasma](https://wiki.archlinux.org/title/KDE).
- [NetworkManager: rc-manager и runtime resolv.conf](https://networkmanager.dev/docs/api/1.48/NetworkManager.conf.html).

Точные пакетные версии берутся из зафиксированного архива и фактических lock-файлов,
а не из текущего содержания этих страниц.
