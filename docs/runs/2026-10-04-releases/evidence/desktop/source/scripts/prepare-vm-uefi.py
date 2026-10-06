#!/usr/bin/env python3
"""Prepare inactive libvirt XML; applying it is a separate explicit operation."""
import argparse
from pathlib import Path
import xml.etree.ElementTree as ET

parser = argparse.ArgumentParser()
parser.add_argument("input", type=Path)
parser.add_argument("output", type=Path)
parser.add_argument("--mac", required=True)
parser.add_argument("--code", default="/usr/share/edk2/x64/OVMF_CODE.4m.fd")
parser.add_argument("--vars", default="/usr/share/edk2/x64/OVMF_VARS.4m.fd")
args = parser.parse_args()
tree = ET.parse(args.input)
domain = tree.getroot()
if not any(mac.get("address", "").lower() == args.mac.lower()
           for mac in domain.findall("./devices/interface/mac")):
    parser.error("The input domain does not have the expected VM MAC address")
for firmware in (args.code, args.vars):
    if not Path(firmware).is_file():
        parser.error(f"Missing firmware: {firmware}")
os_node = domain.find("os")
if os_node is None:
    parser.error("Missing OS definition")
for tag in ("loader", "nvram", "firmware", "boot"):
    for node in os_node.findall(tag):
        os_node.remove(node)
os_node.attrib.pop("firmware", None)
ET.SubElement(os_node, "loader", {
    "readonly": "yes", "secure": "no", "type": "pflash", "format": "raw",
}).text = args.code
name = domain.findtext("name")
ET.SubElement(os_node, "nvram", {
    "template": args.vars, "templateFormat": "raw", "format": "raw",
}).text = f"/var/lib/libvirt/qemu/nvram/{name}_looom_VARS.fd"
for disk in domain.findall("./devices/disk"):
    target = disk.find("target")
    boot = disk.find("boot")
    if boot is not None:
        disk.remove(boot)
    if disk.get("device") == "disk" and target is not None and target.get("dev") == "vda":
        ET.SubElement(disk, "boot", {"order": "1"})
    elif disk.get("device") == "cdrom":
        ET.SubElement(disk, "boot", {"order": "2"})
ET.indent(tree, space="  ")
tree.write(args.output, encoding="unicode")
print(f"Prepared UEFI without Secure Boot, disk first: {args.output}")
