# Испытания обновления и прерываний на VM

Дата: 2026-10-08. Обновление и выбранные fault-сценарии проверены;
найденные проблемы FAT-публикации исправлены. Пределы приёмки указаны ниже.

Среда: исходная разрешённая QEMU/KVM VM `Cachyos`, 6 vCPU, 8 GiB RAM,
UEFI без Secure Boot, виртуальный диск 60 GiB, ESP 2 GiB, Btrfs 58 GiB.
Начальное состояние: running/saved `rust-final`, пустой next, ядро
`7.2.8-arch1-2`, около 27.5 GiB свободно на Btrfs, 827 MiB на ESP.
Физический N100 в этих испытаниях не изменяется.

Все Rust-сборки, форматирование, Clippy и тесты выполняются исключительно
в writable toolchain VM. На клиенте — редактор, Git, доставка файлов через SSH
и управление питанием разрешённой VM через libvirt.

## Цели

1. Обновить весь набор Arch с `2026-10-03` на `2026-10-07`, действительно
   загрузить новый релиз, проверить пакетный состав, откат и сохранение состояния.
2. Проверить настоящее Btrfs ENOSPC после записи меню на отдельный FAT.
3. Жёстко остановить VM в известных точках публикации; после запуска проверить
   сохранность прежнего выбора, удалить незавершённый UKI через журнал,
   восстановить меню и явно повторить публикацию.
4. Прервать реальную финализацию корня и восстановить её замороженным бинарником.

## Изменения

Временное имя UKI теперь записывается в закрытый журнал
`State/publications/<release>.json` до копирования. `recover` удаляет только
названный журналом обычный root-owned файл `.uki-*`; traversal и симлинки
отвергаются. Повторная публикация и GC также согласуют этот журнал.
Меню восстанавливается по метаданным: незавершённый validated-кандидат
не получает автоматического выбора или подтверждения.

Fault injection: `LOOOM_FAIL_AFTER=uki-copy|uki|menu|snapshot`;
обычный режим возвращает ошибку, `LOOOM_FAIL_MODE=stop` останавливает процесс
через SIGSTOP. SIGCONT разрешает продолжить операцию для проверки ENOSPC;
жёсткое выключение выполняется гипервизором. Эти переменные предназначены
только для контролируемых испытаний от root.

`looom-vm-durability` ограничен QEMU/KVM и MAC исходной тестовой VM.
Файлы образов хранятся в закрытом `State/dev/durability-20261008/fixture`:
512 MiB Btrfs + 512 MiB FAT. Тестовые UKI — синтетические данные; они проверяют
порядок файловых операций и не предназначены для загрузки.
Загрузку настоящего UKI проверяет отдельно реальное обновление VM.

Команды harness: `prepare`, `publish`, `check`, `recover`, `finish`,
`enospc`, `clean`. `clean` сначала размонтирует файловые системы, затем
удаляет только известные файлы образов и профиля, без рекурсивного удаления
точек монтирования.

Полное физическое обесточивание накопителя этими испытаниями не моделируется:
жёсткая остановка VM теряет guest RAM и не выполняет guest shutdown,
но сохраняет поведение дискового кеша QEMU и хоста.

## Обнаруженные сбои и исправления

### Два слоя тестовых образов

Первый опыт потерял исходные FAT-каталоги. `syncfs` смонтированного loop-FAT
не давал в нашем стенде достаточного checkpoint backing-файла на внешнем
Btrfs. Исправлена подготовка: после syncfs обоих образов выполняются fsync
backing-файлов, fsync каталога и syncfs внешнего Btrfs. После этого отдельный
опыт в середине копирования сохранил исходные UKI/GRUB/root/метаданные.
Исходный неудачный опыт сохранён отдельно и не засчитан как PASS.

### Повреждение FAT-меню и UKI

Настоящее отключение после записи меню показало, что FAT может сохранить
новое имя `grub.cfg`, но потерять цепочку кластеров. `fsck.fat` усекла его
до нуля. Кандидат UKI также оказался с неправильной контрольной суммой.
Сохранность Btrfs и прежнего выбора не делает такой внешний файл атомарным
при потере питания.

Исправления:

- В самостоятельный EFI-образ GRUB встроена первая запись `looom-emergency`,
  загружающая bootstrap без внешнего меню. Затем через `source` читается
  обычный `grub.cfg`. При нормальном меню работают прежние saved/next.
- Новый `bootstrap` устанавливает такой dispatcher сразу, сохраняя
  vendor-generated GRUB как `grubx64.vendor.efi`.
- Для существующей установки `looom boot-recovery` создаёт отдельный
  `EFI/looom/safex64.efi` и UEFI-запись `looom-safe` через create-only.
  BootOrder и старый EFI-загрузчик при этой команде не меняются. SHA нового
  загрузчика и ESP UUID фиксируются в закрытом `State/boot-recovery.json`.
- Повторная публикация пересоздаёт повреждённый UKI из валидированного
  read-only корня только при phase=validated и отсутствии его в saved/next.
  Опубликованный/подтверждённый или выбранный образ автоматически не заменяется.
- `recover` строит меню из Btrfs-метаданных; кандидат не подтверждается
  автоматически. Восстановление меню должно предшествовать проверке его наличия.

Аварийная загрузка проверена на реальной ESP VM: нормальное меню через Boot0006
загрузило `rust-final`; затем внешнее меню намеренно обнулено, и тот же loader
загрузил `@bootstrap`. SSH/NM/guest agent работали, меню восстановлено новым
Rust `recover`, следующий reboot вернул `rust-final`. После этой проверки
Boot0006 поставлен первым, Boot0005 с прежним загрузчиком сохранён.

Пустое меню проверено фактической загрузкой. Это защита от повреждения внешнего
меню при целых dispatcher, bootstrap UKI и ESP, не восстановление утраченной ESP.
Полный recovery/upgrade начального bootstrap по-прежнему отдельная задача.

### Сеть после power cut

После одного жёсткого отключения `rust-final` и KDE загрузились, но NM показывал
Ethernet unavailable, хотя позже guest kernel показывал carrier=1. Сеть
восстановлена через guest agent: restart NetworkManager и активация сохранённого
`looom-vm`. Диагностика сохранена. Это не считается доказательством автоматического
восстановления сети после любого отключения; дальнейшие настоящие перезагрузки
проверяются через SSH.

## Итог обновления

Зафиксирован полный официальный снимок Arch `2026-10-07`: 651 пакет,
37 изменений версий, без добавления/удаления пакетов. Среди изменений — linux,
OpenSSH, QEMU guest agent, GLib и библиотеки GTK. Это смена всего package lock,
а не обновление работающего root через `pacman -Syu`.

`rust-upgrade-1007` собран с ядром `7.2.9-arch1-1`. В фазе configured создан
read-only `@root-rust-upgrade-1007`, процесс остановлен на `snapshot`, затем VM
жёстко выключена. После запуска сохранился `rust-final`; Rust `recover`
вызвал замороженный бинарник из inputs, завершил метаданные и удалил private
build. Публикация и try выполнены отдельно, новый релиз реально загрузился,
проверен и подтверждён.

Проверена последовательность `rust-final` (2026-10-03, 7.2.8) →
`rust-upgrade-1007` (2026-10-07, 7.2.9) → `rust-final` → `rust-upgrade-1007`.
При откате вернулись прежний kernel и весь package inventory. UID/GID 1000,
временный тестовый пароль и маркеры в home/local-etc сохранились. PAM login,
SDDM и sudo принимали правильный пароль и отвергали неправильный.

В новом root проверена настоящая локальная Plasma Wayland codex на seat0,
активные Plasma/KWin/PipeWire/Pulse/WirePlumber. При финальном входе первая
проверка user services была сделана до завершения старта Plasma и отказала;
повтор после старта прошёл. В журнале сохранены оба результата.
Исходный пароль восстановлен побайтно по закрытому original hash, временное
состояние удалено. Hashes/пароль/SSH private keys не экспортировались.

Финальный running/saved — подтверждённый `rust-upgrade-1007`, next пустой,
failed system units отсутствуют. Нулевой plan: package/file/unit changes нет.
GC выполнен только как preview; существующие релизы не удалялись.
Свободно около 26 GiB на Btrfs, 608 MiB на ESP; увеличение диска не требовалось.

SHA установленного и frozen менеджера:
`583f8fe5bc5c7acdfbd2ead2f05cf483022130d85fb067e944e94dcbd85ff9c0`.
SHA обновлённого UKI:
`7be76829b5ab04396a32c47f114c014b54cb37183d09dd59458fcd861706c2f9`.
Аварийный GRUB: `2da4b6d14fdcb862b892cd77165ca906b4752e4c89ce7af1ff43d16dd61d96c8`.

## Покрытие и доказательства

| Проверка | Результат |
| --- | --- |
| Typed YAML, изолированный bootstrap, runtime suite | PASS, 7 tests |
| Последние изменения publication recovery: runtime, all-target Clippy, release build | PASS |
| Traversal/symlink targets журнала | Отказ, посторонний файл цел |
| Повреждённый validated UKI / опубликованный UKI | Первый пересоздаётся; второй сохраняется с отказом |
| Настоящий Btrfs ENOSPC после записи FAT-меню | PASS: старый выбор/метаданные целы, повтор после освобождения места |
| Жёсткое отключение VM в середине копирования UKI | PASS после исправления checkpoint стенда |
| Жёсткое отключение после меню | Обнаружены повреждения FAT; recover и пересоздание uncommitted UKI проверены |
| Реальная загрузка без внешнего меню | PASS: embedded GRUB → bootstrap → SSH → recover → прежний релиз |
| Жёсткое отключение финализации настоящего root | PASS: configured snapshot восстановлен frozen binary |
| Дата Arch 03 → 07 → 03 → 07 | PASS, реальные загрузки и inventories |
| Persistent пароль, home/local-etc, SDDM/sudo | PASS; исходный пароль восстановлен |
| Plasma Wayland и audio services | PASS после готовности пользовательской сессии |
| Отсутствие secrets в публичном комплекте | PASS до удаления временного пароля и после восстановления |

[Полная Rust-проверка](evidence/rust-recovery-checks.log),
[последняя проверка runtime/Clippy/build](evidence/rust-final-checks.log),
[ENOSPC](evidence/btrfs-enospc.log),
[середина копирования](evidence/powercut-uki-copy.log),
[восстановление после меню](evidence/powercut-menu-final.log),
[пустое меню и аварийная загрузка](evidence/boot-recovery-empty-menu.log),
[recover настоящего root](evidence/upgrade-recover.log),
[откат](evidence/upgrade-rollback.log),
[финальный state и source SHA](evidence/native-final-state.json),
[нулевой plan](evidence/upgrade-final-plan.json),
[проверка secrets до restore](evidence/credential-audit-before-restore.log).

Внешний libvirt snapshot `looom-before-durability-20261008` создан после
штатного shutdown до fault-тестов. Новые записи идут в
`/var/lib/libvirt/images/Cachyos-durability-20261008.qcow2`, старый backing
`Cachyos.qcow2` сохранён. NVRAM этим snapshot не копируется; исходные Boot*
variables отдельно сохранены внутри закрытого VM state. Snapshot не экспортирован.

FAT loop-images после выключения проверялись `fsck.fat -a` с допустимым
кодом 0/1; это соответствует отдельному этапу проверки файловой системы перед
монтированием. После восстановления и unmount — `fsck.fat -n` и
`btrfs check --readonly`; прежние root/UKI и saved choice проверяются отдельно.
Это не утверждение об отсутствии повреждений FAT: повреждения действительно
обнаружены, и именно для них добавлен путь аварийного доступа.

## Повторение на исходной VM

Исходники: `State/dev/src`, compiler chroot: `State/dev/root`, `/workspace`.
Они не устанавливаются в действующий read-only root. Compiler chroot должен
быть смонтирован вместе с workspace/cache, как в журнале Rust-переноса.

```bash
# В compiler chroot исходной VM:
cd /workspace
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
LOOOM_TEST_VM=1 cargo test --locked -- --test-threads=1
cargo build --locked --release
```

```bash
# В основной VM; только disposable fixture, не реальная ESP:
/var/lib/looom/dev/src/target/release/looom-vm-durability enospc
/var/lib/looom/dev/src/target/release/looom-vm-durability clean
/var/lib/looom/dev/src/target/release/looom-vm-durability prepare
LOOOM_FAIL_AFTER=menu LOOOM_FAIL_MODE=stop \
  /var/lib/looom/dev/src/target/release/looom-vm-durability publish
```

Последняя команда намеренно останавливается. На гипервизоре убедиться,
что это нужная VM, затем `virsh -c qemu:///system destroy Cachyos` и `start Cachyos`.
После старта, до loop-mount, выполнить fsck FAT-образа с проверкой кода возврата;
`finish` восстанавливает меню, проверяет прежний root/UKI/выбор и явно повторяет
публикацию дважды. После unmount проверить обе файловые системы и вызвать clean.
Точки `uki-copy`, `uki`, `menu` можно выбирать тем же env. Фактически в этом
журнале выключения сделаны на `uki-copy`, `menu` и `snapshot` настоящей сборки.

Для новой даты использовать [конфигурацию](../../../configs/native/upgrade/base.yaml)
и собственный `looom lock`: SHA текущего Rust-бинарника входит в recipe,
образец lock не переносится между разными бинарниками автоматически.
ID `rust-upgrade-1007` уже занят: для повторной сборки выбрать новый ID.

Следующий этап: журналируемое первоначальное подключение и обновление
bootstrap UKI с согласованием machine profile после прерывания. Затем
автоматический установщик. Приложения, CachyOS, шифрование и Secure Boot
этими испытаниями не добавлялись.
