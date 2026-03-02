import { Component, OnInit, inject } from '@angular/core';
import {
  IonApp,
  IonRouterOutlet,
  IonTabs,
  IonTabBar,
  IonTabButton,
  IonIcon,
  IonLabel,
} from '@ionic/angular/standalone';
import { addIcons } from 'ionicons';
import {
  chatbubbleEllipsesOutline,
  listOutline,
  settingsOutline,
} from 'ionicons/icons';
import { PushNotificationService } from './services/push-notification.service';

@Component({
  selector: 'app-root',
  standalone: true,
  imports: [
    IonApp,
    IonRouterOutlet,
    IonTabs,
    IonTabBar,
    IonTabButton,
    IonIcon,
    IonLabel,
  ],
  template: `
    <ion-app>
      <ion-tabs>
        <ion-tab-bar slot="bottom">
          <ion-tab-button tab="chat">
            <ion-icon name="chatbubble-ellipses-outline"></ion-icon>
            <ion-label>Chat</ion-label>
          </ion-tab-button>

          <ion-tab-button tab="sessions">
            <ion-icon name="list-outline"></ion-icon>
            <ion-label>Sessions</ion-label>
          </ion-tab-button>

          <ion-tab-button tab="settings">
            <ion-icon name="settings-outline"></ion-icon>
            <ion-label>Settings</ion-label>
          </ion-tab-button>
        </ion-tab-bar>
      </ion-tabs>
    </ion-app>
  `,
  styles: [
    `
      ion-tab-bar {
        --background: var(--ion-tab-bar-background, #fff);
        border-top: 1px solid var(--ion-color-step-150, #e0e0e0);
      }

      ion-tab-button {
        --color: var(--ion-tab-bar-color, #8a8a8a);
        --color-selected: var(--ion-tab-bar-color-selected, var(--ion-color-primary));
      }
    `,
  ],
})
export class AppComponent implements OnInit {
  private readonly pushService = inject(PushNotificationService);

  constructor() {
    addIcons({
      'chatbubble-ellipses-outline': chatbubbleEllipsesOutline,
      'list-outline': listOutline,
      'settings-outline': settingsOutline,
    });
  }

  async ngOnInit(): Promise<void> {
    await this.pushService.initialize();
  }
}
