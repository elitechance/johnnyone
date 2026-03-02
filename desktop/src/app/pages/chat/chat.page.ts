import { Component, OnInit, OnDestroy, inject } from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import {
  IonHeader,
  IonToolbar,
  IonTitle,
  IonContent,
  IonButtons,
  IonButton,
  IonIcon,
  IonBadge,
  IonChip,
  IonLabel,
} from '@ionic/angular/standalone';
import { Router } from '@angular/router';
import { TauriBridgeService, ConnectionStatus } from '../../services/tauri-bridge.service';
import { Subject, takeUntil, interval } from 'rxjs';

@Component({
  selector: 'app-chat',
  standalone: true,
  imports: [
    CommonModule,
    FormsModule,
    IonHeader,
    IonToolbar,
    IonTitle,
    IonContent,
    IonButtons,
    IonButton,
    IonIcon,
    IonBadge,
    IonChip,
    IonLabel,
  ],
  templateUrl: './chat.page.html',
  styleUrls: ['./chat.page.scss'],
})
export class ChatPage implements OnInit, OnDestroy {
  private readonly tauriBridge = inject(TauriBridgeService);
  private readonly router = inject(Router);
  private readonly destroy$ = new Subject<void>();

  connectionStatus: ConnectionStatus = { connected: false, session_id: null, last_heartbeat: null };
  messages: Array<{ role: 'user' | 'assistant' | 'system'; content: string; timestamp: Date }> = [];
  currentMessage = '';

  ngOnInit(): void {
    this.pollConnectionStatus();
  }

  ngOnDestroy(): void {
    this.destroy$.next();
    this.destroy$.complete();
  }

  private pollConnectionStatus(): void {
    interval(3000)
      .pipe(takeUntil(this.destroy$))
      .subscribe(async () => {
        try {
          this.connectionStatus = await this.tauriBridge.getConnectionStatus();
        } catch {
          this.connectionStatus = { connected: false, session_id: null, last_heartbeat: null };
        }
      });
  }

  async sendMessage(): Promise<void> {
    const text = this.currentMessage.trim();
    if (!text) return;

    this.messages.push({
      role: 'user',
      content: text,
      timestamp: new Date(),
    });
    this.currentMessage = '';

    // The message will be sent through the agent WebSocket connection
    // and responses will arrive via the same channel
    this.messages.push({
      role: 'system',
      content: 'Message sent to agent. Waiting for response...',
      timestamp: new Date(),
    });
  }

  async connectAgent(): Promise<void> {
    try {
      const sessionId = crypto.randomUUID();
      await this.tauriBridge.connectAgent(sessionId);
    } catch (err) {
      console.error('Failed to connect agent:', err);
    }
  }

  async disconnectAgent(): Promise<void> {
    try {
      await this.tauriBridge.disconnectAgent();
    } catch (err) {
      console.error('Failed to disconnect agent:', err);
    }
  }

  navigateTo(path: string): void {
    this.router.navigate([path]);
  }
}
