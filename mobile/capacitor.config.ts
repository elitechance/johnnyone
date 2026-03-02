import type { CapacitorConfig } from '@capacitor/cli';

const config: CapacitorConfig = {
  appId: 'app.johnnyone.mobile',
  appName: 'JohnnyOne',
  webDir: '../dist/mobile/browser',
  plugins: {
    PushNotifications: {
      presentationOptions: ['badge', 'sound', 'alert'],
    },
    Haptics: {},
  },
  server: {
    androidScheme: 'https',
  },
};

export default config;
