// The places of the application one touch apart: the home, the Den and the
// settings, under a bar at the bottom. Everything else opens over them.
import { Tabs } from 'expo-router';

import { TabBar } from '@/ui/TabBar';

export default function TabsLayout() {
  return (
    <Tabs screenOptions={{ headerShown: false }} tabBar={(props) => <TabBar {...props} />}>
      <Tabs.Screen name="index" options={{ title: 'Home' }} />
      <Tabs.Screen name="den" options={{ title: 'The Office' }} />
      <Tabs.Screen name="settings" options={{ title: 'Settings' }} />
    </Tabs>
  );
}
