import { Computer, DiagramText, Ground, INK, ServerRack, Key, LABEL_AT, MODEL_STYLE, MovingPacket, Phone, PORTS, VIEW_BOX } from "../diagram";

interface KeyLabels {
  ciphertext: string;
  keys: string;
  opened: string;
}

export function KeyLive({ label, labels }: { label: string; labels: KeyLabels }) {
  return (
    <svg viewBox={VIEW_BOX} role="img" aria-label={label} focusable="false" className="block h-auto w-full" fill="none" style={MODEL_STYLE}>
      <Ground />
      <path d={`M${PORTS.pc}C250 193 330 156 ${PORTS.relayLeft}M${PORTS.relayRight}C560 162 650 160 ${PORTS.client}`} stroke={INK.fg} strokeWidth={1.15} vectorEffect="non-scaling-stroke" />
      <MovingPacket path={`M${PORTS.pc}C250 193 330 156 ${PORTS.relayLeft}L${PORTS.relayRight}C560 162 650 160 ${PORTS.client}`} rest={{ x: 290, y: 173 }} duration={5.2} />
      <Computer />
      <Phone />
      <ServerRack />
      <g transform="translate(505 90)">
        <circle r={24} fill={INK.bg} stroke={INK.border} strokeWidth={1} />
        <g transform="scale(0.8)"><Key x={0} y={0} slashed /></g>
      </g>
      <DiagramText x={LABEL_AT.relay[0]} y={LABEL_AT.relay[1]} ink="fg" size={11}>Droppen Hub</DiagramText>
      <DiagramText x={LABEL_AT.relay[0]} y={LABEL_AT.relay[1] + 17}>{labels.ciphertext}</DiagramText>
      <Key x={LABEL_AT.pc[0]} y={LABEL_AT.pc[1] - 22} />
      <Key x={LABEL_AT.client[0]} y={LABEL_AT.client[1] - 22} />
      <DiagramText x={LABEL_AT.pc[0]} y={LABEL_AT.pc[1]} className="max-md:hidden">{labels.keys}</DiagramText>
      <DiagramText x={LABEL_AT.client[0]} y={LABEL_AT.client[1]} className="max-md:hidden">{labels.opened}</DiagramText>
    </svg>
  );
}
