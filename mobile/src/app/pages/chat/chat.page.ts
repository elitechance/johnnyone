import { Component, inject, signal, computed, OnInit, OnDestroy } from '@angular/core';
import { CommonModule } from '@angular/common';
import {
  IonHeader,
  IonToolbar,
  IonTitle,
  IonContent,
  IonButtons,
} from '@ionic/angular/standalone';
import { ChatWindowComponent, NodeStatusComponent } from '@johnnyone/ui';
import { AiChatService, ChatMessage } from '@johnnyone/core';
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

  readonly messages = signal<ChatMessage[]>([]);
  readonly isStreaming = signal<boolean>(false);

  ngOnInit(): void {
    this.subscription = this.chatService.messages$.subscribe((msgs) => {
      this.messages.set(msgs);
    });
  }

  ngOnDestroy(): void {
    this.subscription?.unsubscribe();
  }

  async onSend(content: string): Promise<void> {
    if (!content.trim()) return;

    this.isStreaming.set(true);

    try {
      await this.chatService.sendMessage(content);
    } catch (error) {
      console.error('Failed to send message:', error);
    } finally {
      this.isStreaming.set(false);
    }
  }
}
