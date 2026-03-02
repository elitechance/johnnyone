import { Component, Input, ChangeDetectionStrategy } from '@angular/core';
import { CommonModule } from '@angular/common';
import { IonicModule } from '@ionic/angular';
import { DesktopNode } from '../../models/desktop-node.model';

@Component({
  selector: 'johnny-node-status',
  standalone: true,
  imports: [CommonModule, IonicModule],
  templateUrl: './node-status.component.html',
  styleUrls: ['./node-status.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class NodeStatusComponent {
  @Input() node: DesktopNode | null = null;

  get statusColor(): string {
    if (!this.node) return 'medium';
    switch (this.node.status) {
      case 'online':
        return 'success';
      case 'offline':
        return 'medium';
      case 'busy':
        return 'warning';
      default:
        return 'medium';
    }
  }

  get statusDotClass(): string {
    if (!this.node) return 'dot-offline';
    switch (this.node.status) {
      case 'online':
        return 'dot-online';
      case 'offline':
        return 'dot-offline';
      case 'busy':
        return 'dot-busy';
      default:
        return 'dot-offline';
    }
  }

  get displayName(): string {
    if (!this.node) return 'No node';
    return this.node.hostname;
  }

  get statusLabel(): string {
    if (!this.node) return 'Disconnected';
    return this.node.status.charAt(0).toUpperCase() + this.node.status.slice(1);
  }

  get osIcon(): string {
    if (!this.node) return 'desktop-outline';
    const os = this.node.os.toLowerCase();
    if (os.includes('windows') || os.includes('win32')) return 'logo-windows';
    if (os.includes('mac') || os.includes('darwin')) return 'logo-apple';
    if (os.includes('linux')) return 'logo-tux';
    return 'desktop-outline';
  }

  get lastSeenText(): string {
    if (!this.node?.lastHeartbeatAt) return '';
    const date = new Date(this.node.lastHeartbeatAt);
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffSec = Math.floor(diffMs / 1000);

    if (diffSec < 60) return 'Just now';
    if (diffSec < 3600) return `${Math.floor(diffSec / 60)}m ago`;
    if (diffSec < 86400) return `${Math.floor(diffSec / 3600)}h ago`;
    return date.toLocaleDateString([], { month: 'short', day: 'numeric' });
  }

  get capabilitiesText(): string {
    if (!this.node?.capabilities?.length) return '';
    return this.node.capabilities.join(', ');
  }
}
