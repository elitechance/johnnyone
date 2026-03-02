import { Component, inject, signal, OnInit, OnDestroy } from '@angular/core';
import { CommonModule } from '@angular/common';
import {
  IonHeader,
  IonToolbar,
  IonTitle,
  IonContent,
  IonButtons,
} from '@ionic/angular/standalone';
import {
  ChatWindowComponent,
  NodeStatusComponent,
  AiChatService,
  AiMessage,
} from '@johnnyone/ui';
import { Subscription } from 'rxjs';

@Component({
  selector: 'app-chat',
  standalone: true,
  imports: [
    CommonModule,
    IonHeader,
    IonToolbar,
    IonTitle,
    IonContent,
    IonButtons,
    ChatWindowComponent,
    NodeStatusComponent,
  ],
  templateUrl: './chat.page.html',
  styleUrls: ['./chat.page.scss'],
})
export class ChatPage implements OnInit, OnDestroy {
  private readonly chatService = inject(AiChatService);
  private subscription: Subscription | null = null;

  readonly messages = signal<AiMessage[]>([]);
  readonly isStreaming = signal<boolean>(false);

  ngOnInit(): void {
    // Subscribe to chat service messages when available
  }

  ngOnDestroy(): void {
    this.subscription?.unsubscribe();
  }

  async onSend(content: string): Promise<void> {
    if (!content.trim()) return;

    this.isStreaming.set(true);

    try {
      this.chatService.sendMessage(content);
    } catch (error) {
      console.error('Failed to send message:', error);
    } finally {
      this.isStreaming.set(false);
    }
  }
}
