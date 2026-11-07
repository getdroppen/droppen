import { Computer, DiagramText, Ground, INK, LABEL_AT, MODEL_STYLE, MovingPacket, Phone, PORTS, ServerRack, VIEW_BOX } from "../diagram";

interface NetworkLabels {
  direct: string;
  relay: string;
  relayName: string;
  computer: string;
  phone: string;
}

export function NetworkLive({ label, labels }: { label: string; labels: NetworkLabels }) {
  return (
    <svg viewBox={VIEW_BOX} role="img" aria-label={label} focusable="false" className="block h-auto w-full" fill="none" style={MODEL_STYLE}>
      <Ground />
      <path d={`M${PORTS.pc}C220 193 240 180 265 174M295 168C330 160 360 156 ${PORTS.relayLeft}M${PORTS.relayRight}C560 162 650 160 ${PORTS.client}`} stroke={INK.muted} strokeWidth={1.1} strokeDasharray="4 4" vectorEffect="non-scaling-stroke" />
      <path d="M270 158v20M290 154v20" stroke={INK.danger} strokeWidth={1.4} vectorEffect="non-scaling-stroke" />
      <DiagramText x={280} y={143} ink="fg" className="max-md:hidden">{labels.direct}</DiagramText>
      <path d={`M${PORTS.pc}C250 193 285 220 350 220C395 220 390 156 ${PORTS.relayLeft}M${PORTS.relayRight}C510 162 545 205 610 205C670 205 690 160 ${PORTS.client}`} stroke={INK.fg} strokeWidth={1.15} vectorEffect="non-scaling-stroke" />
      <MovingPacket path={`M${PORTS.pc}C250 193 285 220 350 220C395 220 390 156 ${PORTS.relayLeft}L${PORTS.relayRight}C510 162 545 205 610 205C670 205 690 160 ${PORTS.client}`} rest={{ x: 320, y: 220 }} duration={4.4} />
      <Computer />
      <Phone />
      <ServerRack />
      <DiagramText x={LABEL_AT.pc[0]} y={LABEL_AT.pc[1]} ink="fg" size={11}>{labels.computer}</DiagramText>
      <DiagramText x={LABEL_AT.client[0]} y={LABEL_AT.client[1]} ink="fg" size={11}>{labels.phone}</DiagramText>
      <DiagramText x={LABEL_AT.relay[0]} y={LABEL_AT.relay[1]} ink="fg" size={11}>{labels.relayName}</DiagramText>
      <DiagramText x={LABEL_AT.relay[0]} y={LABEL_AT.relay[1] + 17}>{labels.relay}</DiagramText>
    </svg>
  );
}
