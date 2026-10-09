# Восстановление и обновление bootstrap

Дата: 2026-10-09. Стенд — разрешённая QEMU/KVM VM `Cachyos`, UEFI без
Secure Boot, 60 GiB (ESP 2 GiB + Btrfs 58 GiB), 6 vCPU / 8 GiB RAM.
Исходный running/saved: `rust-upgrade-1007`, linux `7.2.9-arch1-1`.
N100 не изменяется. Rust build, format, Clippy и тесты выполняются только на VM.

## Протокол

Начальная команда `bootstrap <base.yaml>` сохраняет закрытый журнал
`State/bootstrap-install/journal.json`: декларацию (SHA-256), UUID, исходный
root, ядро, оригинальный fstab и контрольные суммы EFI. Образы сначала
создаются на Btrfs, затем копируются на FAT. Та же команда с той же
декларацией и mounts возобновляет операцию; другая декларация отвергается.
Завершённый повтор проверяет профиль и не меняет выбор загрузки. Наличие
прежней инфраструктуры без подходящего журнала требует отдельного разбора:
чужие каталоги загрузчика автоматически не присваиваются и не удаляются.

`bootstrap-update <confirmed-running-release>` работает из здорового
подтверждённого read-only релиза, выбранного saved. Создаёт writable
`@bootstrap-<release>`, сохраняет собственные пакеты/locked account templates,
bind mounts, новый native manager и его исходники. Desktop в аварийном root
не запускается автоматически (multi-user.target), пакеты остаются доступны.
Первоначальный bootstrap root не изменяется.

Журнал `State/bootstrap-update/journal.json` имеет фазы staging, ready,
committing, complete. В этой закрытой директории остаются candidate.efi и
previous.efi. Готовый кандидат сам не получает загрузочный выбор.
`bootstrap-try` добавляет отдельный UKI и one-shot entry;
`bootstrap-confirm` разрешена только после фактической загрузки этого root,
проверки ядра, manager, постоянных mounts, системных служб и учётных записей.
Saved рабочий релиз не меняется.

Перед заменой фиксированного аварийного UKI подтверждение сохраняет committing.
`bootstrap-recover` согласует известные старый/новый профиль и образ даже
в окне между записью на FAT и Btrfs. Обычный `recover` вызывает этот шаг до
строгой проверки bootstrap SHA. Восстановление ready/staging не подтверждает
и не выбирает кандидата. Прежний UKI и исходный root сохраняются; внешнее меню
содержит отдельную previous bootstrap entry после завершения.

В MVP сохраняется один журнал обновления и его обе копии. Повтор этой же
операции идемпотентен. Следующая смена аварийной базы и очистка истории требуют
отдельного управления поколениями; автоматического удаления backup пока нет.

## Воспроизведение

Использовать собранный на VM актуальный бинарник из writable toolchain.
Исходный работающий release manager остаётся read-only и не заменяется.

```sh
looom bootstrap-update rust-upgrade-1007
looom bootstrap-try
systemctl reboot
# После загрузки аварийного кандидата:
looom bootstrap-confirm
# Следующий обычный reboot возвращает сохранённый рабочий релиз.
```

При прерывании подтверждения выполнить актуальным native бинарником
`looom bootstrap-recover` либо `looom recover`. Пароли и SSH keys в публичные
артефакты не копировать. State, резервные root и credential hash — закрытые
ресурсы. Изменение старого подготовленного /etc/shadow во время незавершённого
первого импорта приводит к отказу согласования, а не перезаписи сохранённого хэша.

Fault points первого bootstrap: `bootstrap-journal`, `bootstrap-uki`,
`bootstrap-loader`, `bootstrap-credentials`, `bootstrap-import`,
`bootstrap-profile`, `bootstrap-menu`.
Обновление: `bootstrap-update-journal`, `bootstrap-update-snapshot`,
`bootstrap-update-ready`, `bootstrap-update-commit`, `bootstrap-update-uki`,
`bootstrap-update-profile`. `LOOOM_FAIL_AFTER=<point>` возвращает ошибку;
с `LOOOM_FAIL_MODE=stop` останавливает процесс для жёсткого выключения VM.
Это средства испытаний от root.

## Пределы

Начальные import checkpoints проверяются на отдельном FAT и отдельных
подтомах VM. Прерывания initial bootstrap пока моделируются возвратом ошибки.
Жёсткое отключение VM сохраняет свойства кеша QEMU/хоста и не равно
физическому обесточиванию накопителя. Журнал помогает восстановить FAT из
Btrfs; потерю самой ESP/Btrfs он не исправляет. Во время прерванной замены
стабильного UKI для восстановления может потребоваться рабочий release,
previous entry либо live ISO. Встроенная emergency entry сама образ не чинит.

## Результаты

Подтверждено на VM:

- Format, Clippy (`--all-targets -D warnings`), release build и все 7 тестов.
- Первый bootstrap: семь прерываний на изолированном FAT/подтомах,
  сохранение импортированных credentials/локального файла, повторение без
  изменения загрузочного выбора. Проверяется и валидность альтернативной
  декларации до её ожидаемого отказа при возобновлении.
- Частичный scratch удаляется при повторе; symlink вместо Btrfs-checkpoint
  отвергается, credential target остаётся прежним. Первый вариант этого
  теста пытался создать symlink на FAT и получил EPERM: тест перенесён
  на Btrfs; исходный неудачный прогон сохранён, не считается PASS.
- Настоящая подготовка нового `@bootstrap-rust-upgrade-1007` пережила
  прерывания journal, snapshot и ready; в ready не меняются saved/next.
- Семь некорректных журналов (traversal, schema, phase, digest, identity,
  unknown field) отвергнуты без изменения machine profile, аварийного UKI
  и GRUB environment. Подтверждение из рабочего релиза отвергается.

Первый кандидат фактически загружен: `@bootstrap-rust-upgrade-1007`, rw,
ядро `7.2.9-arch1-1`, sshd/NM/accounts/guest-agent active, failed units нет.
One-shot потреблён, saved остаётся `rust-upgrade-1007`; login PAM принимает
сохраняемый тестовый пароль и отвергает неправильный. Ethernet поднялся сам.
После reboot без подтверждения VM вернулась в read-only `rust-upgrade-1007`,
прошли native verify и PAM. После второй реальной загрузки health gate дошёл до committing и был прерван.
`bootstrap-recover` остановлен SIGSTOP после записи UKI/syncfs ESP, до записи
machine profile. Гипервизор жёстко остановил VM и запустил её вновь.
Saved read-only релиз загрузился, Ethernet активировался автоматически.
Старый `verify` корректно отверг расхождение bootstrap UKI/profile.
Новый обычный `recover` дошёл до profile и был ещё раз прерван; повтор завершил
согласование и восстановил меню. `verify`, PAM и идемпотентный повтор обновления
прошли. Saved/next и рабочий root сохранились. Прежний root не изменялся.

Подтверждённый новый UKI через стабильный путь загрузил тот же новый rw root
с ядром 7.2.9, рабочими SSH/NM/accounts/guest-agent и сохранённым паролем.
Исходный пароль восстановлен, временное password-test state удалено.
Отдельная previous entry действительно загрузила исходный `@bootstrap`
с ядром 7.2.8; SSH/NM/guest-agent работают. Дальнейший reboot возвращает saved
рабочий релиз. Старый root и старый UKI не пересоздавались.

## Новый рабочий релиз

Профиль `configs/native/recovery/base.yaml` сохраняет полный Arch snapshot
2026-10-07 и KDE. Разрешён новый lock (651 подписанный пакет) для актуального
Rust manager SHA `a564be970ee86cd31ac3dc9a99cea13502f4c64976166133611218b5dc38f8c8`.
Собран и валидирован read-only `rust-bootstrap-recovery`; его публикация
и one-shot trial выполняются отдельно от сборки.
Для места на ESP VM-only helper удалил только уже не выбранный и отсутствующий
в меню trial slot: 225111040 bytes. Stable/previous UKI, Btrfs checkpoints
и оба root сохранены; это очистка стенда, не общий production GC bootstrap.
После неё ESP имела 393 MiB свободно; на Btrfs после новой сборки около 23 GiB.

Новый рабочий root действительно загружен read-only. Native verify проверил
UKI/kernel, mounts, учётные записи, точный пакетный состав и замороженный manager.
PAM login принимает сохраняемый пароль и отвергает неправильный. В реальной
локальной сессии codex (UID 1000, seat0, Wayland) активны Plasma/KWin, PipeWire,
Pulse и WirePlumber. Релиз подтверждён; затем выполнен rollback в
`rust-upgrade-1007`, где также прошли verify/PAM. Возврат выбирается прежним
Rust manager через rollback в уже подтверждённый новый релиз.

## Итоговое состояние и артефакты

VM оставлена в `rust-bootstrap-recovery`, confirmed/running/saved, next пуст.
Root read-only, linux 7.2.9-arch1-1, 651 пакет, KDE Wayland активна; failed units
нет. Исходные root/user credentials совпадают с закрытым baseline, runtime
shadow согласован, временное password-test state удалено. Toolchain размонтирован.
Native report проверил 14 файлов frozen source нового bootstrap против
проверенного исходного дерева. Manager SHA:
`a564be970ee86cd31ac3dc9a99cea13502f4c64976166133611218b5dc38f8c8`.
Рабочий UKI SHA:
`f5c1e65ce752f94defa2b1527b77cf751fcf6fcee6af2eb6e0a24dc6241cbf3d`.
Новый bootstrap UKI SHA:
`e43806d8a8bf0cd314cca682ac3dae7c73eba2f140b9bd5477c7a8f212f1666a`.
Прежний bootstrap UKI SHA:
`a6ee510c23e0ef61e46055a35cd1b67b92808b6e819ef2de9ce39344f506b9a5`.

Публичный набор содержит 60 обычных файлов: логи проверок и всех прерываний,
состояния ready/committing/complete, окончательный отчёт и manifest SHA-256.
Аудит до удаления обоих тестовых паролей и финальный аудит отвергли бы
реальные credential hashes/пароли в экспорте. Только этот набор из
`State/dev/bootstrap-20261009/public` экспортирован в [evidence](evidence/);
закрытые baseline, passwords, SSH keys, root и EFI backups на клиент не переносились.
Контрольные суммы: [public-artifacts.sha256](evidence/public-artifacts.sha256).
Главный отчёт: [final-state.json](evidence/final-state.json).

Сеть на проверенных стартах (включая hard cut и старый bootstrap) поднялась
автоматически. Профиль looom-vm: autoconnect=yes, DHCP, NetworkManager 1.58.1-2.
Эта версия NM совпадает с прежней, где однажды наблюдался unavailable при
carrier=1; причина того эпизода не установлена. Нового случая и ручного restart
NetworkManager в этом цикле не было. Это наблюдение, а не доказательство
исправления прежнего редкого сбоя. Полные boot-логи сохранены.

Диск увеличивать для этого этапа не потребовалось: Btrfs около 23 GiB free,
ESP 178 MiB free после публикации нового UKI. Перед следующими публикациями
нужна плановая очистка старых release EFI/roots либо расширение раскладки;
старые fixtures на этом этапе не удалялись. Управление историей bootstrap
и автоматический установщик — дальнейшие этапы. Initial bootstrap/init
в MVP выполнять последовательно, без параллельного build/publish.
